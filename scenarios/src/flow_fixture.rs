//! In-memory UI, daemon-event and wallet boundaries for scenario
//! sequencing: a daemon that publishes what the real one publishes, two
//! silent actors, and wallets that answer for the payments the daemon
//! made or returned.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use async_trait::async_trait;
use mortsom_app_contract::{FiatAmount, OrderDraft, OrderKind, OrderPrice, OrderState};
use mortsom_common::Actor;
use mortsom_common::{Lane, Secret};
use mortsom_observers::{
    relay::{ObserverScope, RelayObserver},
    wallet::{PaymentState, WalletObserver, WalletReader, payment_hash_of},
};
use mortsom_scenario_kit::context::{Context, HoldInvoiceState};
use mortsom_scenario_kit::{
    budget::{Budget, SpendCaps},
    cleanup::CleanupRegistry,
    context::{ActorSession, MainnetClearance, NodeWallet, RecoveryTiming},
    evidence::NullSink,
    recovery::NoRelayControl,
    timeline::Timeline,
};
use mortsom_ui_driver::ui::tests_support::ScriptedUi;
use nostr_sdk::prelude::{EventBuilder, FinalizeEvent, Keys, Kind, Tag};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{Arc, Mutex},
    time::Duration,
};

#[path = "flow_test_adapter.rs"]
mod adapter;
pub use adapter::Adapter;

/// The order every scenario creates first; later ones are numbered.
pub const ORDER: &str = "the-order";
/// The remainder the daemon spawns from a range order.
pub const REMAINDER: &str = "the-remainder";
/// The dispute a party opens on the order.
pub const DISPUTE: &str = "the-dispute";
/// The sats every fixture order is for.
pub const SATS: u64 = 250_000;
/// The buyer invoice lifetime the fixture daemon demands, in seconds.
pub const EXPIRY_FLOOR_SECS: u64 = 300;
/// How many unresolved payouts the fixture daemon lets one payee hold.
pub const PAYOUT_CAP: u32 = 2;

/// How the fixture daemon deviates from the well-behaved one.
#[derive(Debug, Clone, Copy, Default)]
pub struct Misbehaviour {
    /// The seller's hold payment fails.
    pub fail_payment: bool,
    /// A lone cancel request on a funded order closes it outright.
    pub lone_cancel_closes: bool,
    /// A market order is published with a sats amount.
    pub prices_market_orders_upfront: bool,
    /// A dispute is published as opened by the other side.
    pub swaps_dispute_initiator: bool,
    /// The solver's take is answered with some other order's id.
    pub solver_reports_another_order: bool,
    /// A payout to a withdrawn invoice settles anyway.
    pub pays_withdrawn_invoices: bool,
    /// A buyer invoice below the expiry floor is accepted by the daemon.
    pub accepts_short_invoices: bool,
    /// The node's capabilities reach the app too late to judge a short
    /// invoice, which then goes to the daemon.
    pub capabilities_arrive_late: bool,
    /// The app refuses a short invoice naming some other rule.
    pub refuses_for_another_rule: bool,
    /// The app never publishes `valid` for a proper invoice.
    pub never_judges_valid: bool,
    /// Payouts to one destination are dispatched past the cap.
    pub ignores_payout_cap: bool,
    /// A cancel of an order that never went active is never published:
    /// the app drops the trade all the same.
    pub never_publishes_cancel: bool,
}

/// What the fixture knows about an invoice one of its nodes issued.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IssuedInvoice {
    /// Who issued it.
    pub actor: Actor,
    /// The order it pays, once handed in or paid.
    pub order: Option<String>,
    /// Whether it is a hold invoice, and where it stands.
    pub hold: Option<HoldInvoiceState>,
    /// Cancelled on the node.
    pub superseded: bool,
    /// Settled for good: the daemon paid it (a payout invoice), or the
    /// escrow it secures settled at the release (Alice's hold invoice).
    pub settled: bool,
}

pub(crate) struct State {
    orders: BTreeMap<String, OrderState>,
    created: Option<(Actor, OrderKind)>,
    draft: Option<OrderDraft>,
    actions: Vec<String>,
    wallet_checks: Vec<String>,
    /// Orders whose hold Alice has paid.
    funded: BTreeSet<String>,
    refunded: bool,
    taken_at: Option<u64>,
    remainder: Option<OrderState>,
    cancel_requests: Vec<Actor>,
    rated: Vec<Actor>,
    /// What the daemon publishes when a take is abandoned, if anything.
    reaps_abandoned_take_to: Option<&'static str>,
    /// The daemon expires an order nobody takes.
    expires_pending: bool,
    /// The daemon cancels a funded take whose buyer never invoices, and
    /// returns the escrow.
    cancels_funded_take: bool,
    /// Orders the fixture daemon closed on its own, dropped from the app's
    /// list the way app v2 drops a never-active trade.
    dropped: Vec<String>,
    /// Who opened the dispute on the order, while one is open.
    dispute_initiator: Option<&'static str>,
    /// Every invoice a fixture node issued, by payment hash.
    invoices: BTreeMap<String, IssuedInvoice>,
    /// The buyer invoice the daemon holds for each order.
    buyer_invoice: BTreeMap<String, String>,
    /// Orders the daemon is asking a replacement invoice for.
    asks_invoice: BTreeSet<String>,
    /// Payouts held back by the per-destination cap, in dispatch order.
    gated: Vec<String>,
    /// Invoices issued so far, for distinct hashes.
    issued: u64,
    /// The fixture daemon's relay and keys, for the wallet side, which
    /// publishes when a held payout settles.
    daemon: Option<(Arc<RelayObserver>, Keys)>,
    misbehaviour: Misbehaviour,
}

impl State {
    pub(crate) fn order(&self, id: &str) -> OrderState {
        self.orders
            .get(id)
            .copied()
            .unwrap_or_else(|| panic!("the fixture never created order {id}"))
    }
    pub(crate) fn set(&mut self, id: &str, state: OrderState) {
        self.orders.insert(id.to_owned(), state);
    }
    /// The id of the order created after `count` others.
    pub(crate) fn order_name(count: usize) -> String {
        if count == 0 {
            ORDER.to_owned()
        } else {
            format!("{ORDER}-{}", count + 1)
        }
    }
    pub(crate) fn invoice_mut(&mut self, hash: &str) -> &mut IssuedInvoice {
        self.invoices
            .get_mut(hash)
            .unwrap_or_else(|| panic!("no fixture node issued invoice {hash}"))
    }
    /// How many payouts the buyer's node is holding unresolved.
    pub(crate) fn held_payouts(&self) -> usize {
        self.invoices
            .values()
            .filter(|invoice| invoice.hold == Some(HoldInvoiceState::Accepted))
            .count()
    }
}

/// A real, signed BOLT 11 invoice for `msats` on regtest with the given
/// lifetime; `seed` makes the hash unique. Never paid anywhere.
pub fn build_invoice(msats: u64, expiry_secs: u64, seed: u64) -> String {
    use bitcoin::hashes::{Hash, sha256};
    use bitcoin::secp256k1::{Secp256k1, SecretKey};
    use lightning_invoice::{Currency, InvoiceBuilder, PaymentSecret};
    let secp = Secp256k1::new();
    let key = SecretKey::from_slice(&[7u8; 32]).unwrap();
    let preimage = sha256::Hash::hash(&seed.to_be_bytes());
    let hash = sha256::Hash::hash(preimage.as_ref());
    InvoiceBuilder::new(Currency::Regtest)
        .description(format!("fixture {seed}"))
        .payment_hash(hash)
        .payment_secret(PaymentSecret([9u8; 32]))
        .amount_milli_satoshis(msats)
        .duration_since_epoch(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap(),
        )
        .min_final_cltv_expiry_delta(18)
        .expiry_time(Duration::from_secs(expiry_secs))
        .build_signed(|message| secp.sign_ecdsa_recoverable(message, &key))
        .unwrap()
        .to_string()
}

pub struct Fixture {
    pub context: Context,
    pub adapter: Arc<Adapter>,
    state: Arc<Mutex<State>>,
}

impl Fixture {
    pub async fn new(fail_payment: bool) -> Self {
        Self::with(Misbehaviour {
            fail_payment,
            ..Misbehaviour::default()
        })
        .await
    }

    pub async fn with(misbehaviour: Misbehaviour) -> Self {
        let keys = Keys::generate();
        let mut scope = ObserverScope::daemon_only(keys.public_key());
        scope.add_order(ORDER);
        let relay = Arc::new(RelayObserver::detached(scope));
        let state = Arc::new(Mutex::new(State {
            orders: BTreeMap::new(),
            created: None,
            draft: None,
            actions: Vec::new(),
            wallet_checks: Vec::new(),
            funded: BTreeSet::new(),
            refunded: false,
            taken_at: None,
            remainder: None,
            cancel_requests: Vec::new(),
            rated: Vec::new(),
            reaps_abandoned_take_to: None,
            expires_pending: false,
            cancels_funded_take: false,
            dropped: Vec::new(),
            dispute_initiator: None,
            invoices: BTreeMap::new(),
            buyer_invoice: BTreeMap::new(),
            asks_invoice: BTreeSet::new(),
            gated: Vec::new(),
            issued: 0,
            daemon: Some((relay.clone(), keys.clone())),
            misbehaviour,
        }));
        let adapter = Arc::new(Adapter {
            state: state.clone(),
            relay: relay.clone(),
            keys,
        });
        let actors = Actor::ALL
            .into_iter()
            .map(|actor| ActorSession {
                actor,
                ui: Arc::new(ScriptedUi::default()),
                wallet_label: actor.as_str().into(),
            })
            .collect();
        let wallets = Actor::ALL
            .into_iter()
            .map(|actor| {
                (
                    actor.as_str().into(),
                    Arc::new(Wallet {
                        actor,
                        state: state.clone(),
                    }) as Arc<dyn WalletReader>,
                )
            })
            .collect();
        let node_wallets = Actor::ALL
            .into_iter()
            .map(|actor| {
                (
                    actor.as_str().into(),
                    Arc::new(Wallet {
                        actor,
                        state: state.clone(),
                    }) as Arc<dyn NodeWallet>,
                )
            })
            .collect();
        let context = Context {
            adapter: adapter.clone(),
            actors,
            relay,
            node_wallets,
            wallets: Arc::new(WalletObserver::new(wallets)),
            env: mortsom_app_contract::TestEnvironment {
                mostro_pubkey: adapter.keys.public_key().to_hex(),
                relay_url: "ws://fixture.invalid".into(),
                lane: Lane::Local,
            },
            relay_control: Arc::new(NoRelayControl),
            solver: Some(adapter.clone()),
            budget: Arc::new(Mutex::new(Budget::new(
                SpendCaps::new(SATS, SATS * 10).unwrap(),
            ))),
            cleanup: Arc::new(CleanupRegistry::default()),
            timeline: Arc::new(Timeline::default()),
            evidence: Arc::new(NullSink),
            journal: Arc::new(mortsom_scenario_kit::journal::NoJournal),
            pending_holds: Mutex::new(BTreeMap::new()),
            mainnet: Some(MainnetClearance { lane: Lane::Local }),
            limits: mortsom_scenario_kit::PayoutLimits {
                payout_invoice_min_expiry_secs: Some(EXPIRY_FLOOR_SECS),
                max_inflight_payouts_per_destination: Some(PAYOUT_CAP),
                bond: None,
            },
            amount_sats: SATS,
            fiat_code: "USD".into(),
            fiat_amount: 10,
            payment_method: "Cash".into(),
            ui_timeout: Duration::from_millis(50),
            state_timeout: Duration::from_millis(500),
            settle_timeout: Duration::from_millis(50),
            expiration_timeout: Duration::from_millis(500),
            recovery: RecoveryTiming {
                quiet_window: Duration::from_millis(30),
                background_secs: 0,
                outage: Duration::ZERO,
            },
        };
        Self {
            context,
            adapter,
            state,
        }
    }

    /// The daemon reaps an abandoned take by publishing the order as `wire`.
    pub fn reaps_abandoned_take_to(self, wire: &'static str) -> Self {
        self.state.lock().unwrap().reaps_abandoned_take_to = Some(wire);
        self
    }
    /// The daemon expires the order once nobody has taken it.
    pub fn expires_pending(self) -> Self {
        self.state.lock().unwrap().expires_pending = true;
        self
    }
    /// The daemon cancels a funded take whose buyer never invoices and
    /// returns the escrow.
    pub fn cancels_funded_take(self) -> Self {
        self.state.lock().unwrap().cancels_funded_take = true;
        self
    }

    /// A run that registered no solver.
    pub fn without_solver(mut self) -> Self {
        self.context.solver = None;
        self
    }

    /// A stack that declares neither payout limit.
    pub fn without_payout_limits(mut self) -> Self {
        self.context.limits = mortsom_scenario_kit::PayoutLimits::default();
        self
    }

    pub fn actions(&self) -> Vec<String> {
        self.state.lock().unwrap().actions.clone()
    }
    pub fn wallet_checks(&self) -> Vec<String> {
        self.state.lock().unwrap().wallet_checks.clone()
    }
    /// The invoices Bob's node issued, with what became of each.
    pub fn buyer_invoices(&self) -> Vec<IssuedInvoice> {
        self.state
            .lock()
            .unwrap()
            .invoices
            .values()
            .filter(|invoice| invoice.actor == Actor::Bob)
            .cloned()
            .collect()
    }
    /// Every state the relay saw for `order`, in order.
    pub fn relay_states(&self, order: &str) -> Vec<String> {
        self.context
            .relay
            .snapshot()
            .iter()
            .filter(|e| e.kind == 38383 && e.tag("d") == Some(order))
            .filter_map(|e| e.tag("s").map(str::to_owned))
            .collect()
    }
    /// Every status the relay saw for dispute `dispute`, in order.
    pub fn dispute_states(&self, dispute: &str) -> Vec<String> {
        self.context
            .relay
            .snapshot()
            .iter()
            .filter(|e| {
                e.kind == mortsom_scenario_kit::actions::DISPUTE_EVENT_KIND
                    && e.tag("d") == Some(dispute)
            })
            .filter_map(|e| e.tag("s").map(str::to_owned))
            .collect()
    }
    /// How many reputation events the daemon published.
    pub fn ratings_published(&self) -> usize {
        self.context
            .relay
            .snapshot()
            .iter()
            .filter(|e| e.kind == mortsom_scenario_kit::actions::RATING_EVENT_KIND)
            .count()
    }
}

struct Wallet {
    actor: Actor,
    state: Arc<Mutex<State>>,
}

impl Wallet {
    /// Issues an invoice on this node and remembers it by hash.
    fn issue(&self, sats: u64, expiry_secs: u64, hold: bool) -> Secret {
        assert_eq!(self.actor, Actor::Bob);
        assert_eq!(sats, SATS);
        let mut state = self.state.lock().unwrap();
        // The maker of a buy is asked for the invoice only once the seller's
        // hold is accepted; the taker of a sell hands it in first.
        if matches!(state.created, Some((_, OrderKind::Buy))) {
            assert!(
                !state.funded.is_empty(),
                "buyer invoice must follow accepted escrow"
            );
        }
        state.issued += 1;
        let invoice = Secret::new(build_invoice(sats * 1000, expiry_secs, state.issued));
        let hash = payment_hash_of(&invoice).unwrap();
        state.invoices.insert(
            hash,
            IssuedInvoice {
                actor: Actor::Bob,
                order: None,
                hold: hold.then_some(HoldInvoiceState::Open),
                superseded: false,
                settled: false,
            },
        );
        invoice
    }
}

#[async_trait]
impl NodeWallet for Wallet {
    async fn make_invoice(&self, sats: u64, _: &str) -> Result<Secret, String> {
        Ok(self.issue(sats, 3600, false))
    }
    async fn make_invoice_expiring(
        &self,
        sats: u64,
        _: &str,
        expiry_secs: u64,
    ) -> Result<Secret, String> {
        Ok(self.issue(sats, expiry_secs, false))
    }
    async fn make_hold_invoice(&self, sats: u64, _: &str) -> Result<Secret, String> {
        Ok(self.issue(sats, 3600, true))
    }
    async fn cancel_invoice(&self, hash: &str) -> Result<(), String> {
        let mut state = self.state.lock().unwrap();
        let actor = self.actor;
        state.actions.push(format!("cancel-invoice:{actor}"));
        let invoice = state.invoice_mut(hash);
        assert_eq!(invoice.actor, actor);
        if invoice.settled {
            return Err("a settled invoice cannot be cancelled".into());
        }
        invoice.superseded = true;
        invoice.hold = invoice.hold.map(|_| HoldInvoiceState::Cancelled);
        Ok(())
    }
    async fn settled_record_count(&self, hash: &str, incoming: bool) -> Result<usize, String> {
        assert!(incoming);
        let state = self.state.lock().unwrap();
        Ok(usize::from(state.invoices[hash].settled))
    }
    async fn hold_invoice_state(&self, hash: &str) -> Result<HoldInvoiceState, String> {
        let state = self.state.lock().unwrap();
        state.invoices[hash]
            .hold
            .ok_or_else(|| "not a hold invoice".to_owned())
    }
    async fn settle_hold_invoice(&self, hash: &str) -> Result<(), String> {
        let order = {
            let mut state = self.state.lock().unwrap();
            let actor = self.actor;
            state.actions.push(format!("settle-hold:{actor}"));
            let invoice = state.invoice_mut(hash);
            if invoice.hold != Some(HoldInvoiceState::Accepted) {
                return Err("no held payment to settle".into());
            }
            invoice.hold = Some(HoldInvoiceState::Settled);
            invoice.settled = true;
            invoice
                .order
                .clone()
                .expect("a held payout names its order")
        };
        Adapter::payout_landed(&self.state, &order).await;
        Ok(())
    }
    async fn start_payment(&self, invoice: &Secret) -> Result<(), String> {
        assert_eq!(self.actor, Actor::Alice);
        let hash = payment_hash_of(invoice).unwrap();
        let mut state = self.state.lock().unwrap();
        state.actions.push("pay:alice".into());
        if state.misbehaviour.fail_payment {
            return Err("injected hold payment failure".into());
        }
        let order = state.invoices[&hash]
            .order
            .clone()
            .expect("a hold invoice names its order");
        state.funded.insert(order.clone());
        // On a sell the daemon tells both parties `active` once the hold
        // is accepted; on a buy it first asks the maker for the invoice.
        if matches!(state.created, Some((_, OrderKind::Sell))) {
            state.set(&order, OrderState::Active);
        }
        Ok(())
    }
}
#[async_trait]
impl WalletReader for Wallet {
    async fn lookup(&self, hash: &str) -> Result<Option<(u64, u64, PaymentState)>, String> {
        let mut state = self.state.lock().unwrap();
        let invoice = state.invoices[hash].clone();
        let payment_state = match invoice.actor {
            // Alice's node paid the hold: settled once the daemon settled
            // the escrow at release, returned when the order was cancelled.
            Actor::Alice => {
                assert_eq!(self.actor, Actor::Alice);
                let order = invoice.order.as_deref().expect("hold names its order");
                if state.refunded {
                    PaymentState::Failed
                } else if invoice.settled {
                    PaymentState::Settled
                } else {
                    assert!(state.funded.contains(order));
                    match state.order(order) {
                        OrderState::Completed | OrderState::PayoutPending => PaymentState::Settled,
                        _ => PaymentState::Pending,
                    }
                }
            }
            // Bob's node issued the payout invoice.
            Actor::Bob => {
                assert_eq!(self.actor, Actor::Bob);
                if invoice.settled {
                    PaymentState::Settled
                } else if invoice.superseded {
                    PaymentState::Failed
                } else {
                    PaymentState::Pending
                }
            }
        };
        state.wallet_checks.push(format!(
            "{}:{}",
            self.actor,
            match payment_state {
                PaymentState::Settled => "settled",
                PaymentState::Failed => "failed",
                _ => "pending",
            }
        ));
        Ok(Some((SATS, 0, payment_state)))
    }
}
