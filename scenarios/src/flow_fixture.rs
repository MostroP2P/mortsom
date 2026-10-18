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
use mortsom_scenario_kit::context::{BondApplyTo, BondPolicy, Context, HoldInvoiceState};
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
use adapter::DAEMON_DELAY;

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
/// The bond a bonded fixture daemon asks both sides for: one percent of
/// the order, never under 1000 sats, half of it kept by the node when
/// slashed.
pub const BOND_POLICY: BondPolicy = BondPolicy {
    apply_to: BondApplyTo::Both,
    amount_bps: 100,
    base_amount_sats: 1000,
    slash_on_waiting_timeout: true,
    node_share_bps: 5000,
    payout_claim_window_days: 1,
};

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
    /// The book shows a take as in progress while the bond is still
    /// outstanding.
    pub publishes_take_before_bond_lock: bool,
    /// A bond is settled instead of released when the trade ends well.
    pub slashes_honest_bonds: bool,
    /// The taker's app keeps a take whose bond lapsed unpaid.
    pub keeps_lapsed_takes: bool,
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
    /// The daemon's bond invoice, paid by `actor`; held until the trade
    /// ends and then returned or slashed.
    pub bond: bool,
    /// The winner's invoice for a slashed bond's share, paid by the daemon.
    pub claim: bool,
}

/// A step the daemon hands out and waits for, under a deadline.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Duty {
    /// The buyer's payout invoice.
    BuyerInvoice,
    /// The seller's escrow.
    Escrow,
}

/// A slashed bond's share the fixture daemon offers a winner.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Claim {
    /// The winner.
    pub actor: Actor,
    /// Sats on offer.
    pub amount: u64,
    /// `pending`, `submitted` or `completed`.
    pub phase: String,
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
    /// The daemon asks both sides for a bond.
    bonded: bool,
    /// Orders whose taker's bond is requested and not yet locked.
    bond_requested: BTreeSet<String>,
    /// Orders whose maker's bond is requested: created, not published.
    maker_bond_requested: BTreeSet<String>,
    /// Orders an actor walked away from (a taker's bond, a maker's unpublished
    /// order) or lost to a slash: their app no longer lists them.
    declined: Vec<(Actor, String)>,
    /// Whose bond the daemon slashed on each order.
    slashed: BTreeMap<String, Actor>,
    /// The cause the loser's trade shows for the slash.
    slash_notice: BTreeMap<String, (Actor, &'static str)>,
    /// The share on offer to the winner of each slash.
    claims: BTreeMap<String, Claim>,
    /// The daemon slashes whoever leaves a waiting step undone past its
    /// deadline: the buyer's invoice, the seller's escrow.
    slashes_on_timeout: bool,
    /// The daemon lets an unpaid bond lapse: a maker's order it never
    /// published is dropped, a taker's take ends and the order, `pending`
    /// all along, is republished.
    expires_unpaid_bonds: bool,
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
    /// The taker of the created order: whoever did not create it.
    pub(crate) fn taker(&self) -> Option<Actor> {
        self.created.map(|(maker, _)| match maker {
            Actor::Alice => Actor::Bob,
            Actor::Bob => Actor::Alice,
        })
    }
    /// The maker of the created order.
    pub(crate) fn maker(&self) -> Option<Actor> {
        self.created.map(|(maker, _)| maker)
    }
    /// Who buys on the created order: the maker of a buy, the taker of a sell.
    pub(crate) fn buyer(&self) -> Option<Actor> {
        match self.created?.1 {
            OrderKind::Buy => self.maker(),
            OrderKind::Sell => self.taker(),
        }
    }
    /// The other party of `actor` on the created order.
    pub(crate) fn counterparty(actor: Actor) -> Actor {
        match actor {
            Actor::Alice => Actor::Bob,
            Actor::Bob => Actor::Alice,
        }
    }
    /// The bond invoice `actor` paid on `order`, if any.
    pub(crate) fn bond_invoice_mut(
        &mut self,
        actor: Actor,
        order: &str,
    ) -> Option<&mut IssuedInvoice> {
        self.invoices.values_mut().find(|invoice| {
            invoice.bond && invoice.actor == actor && invoice.order.as_deref() == Some(order)
        })
    }
    /// The daemon slashes `loser`'s bond on `order` for `cause`: the HTLC
    /// settles, the counterparty is offered their share, and the loser's
    /// trade carries the cause.
    pub(crate) fn slash(&mut self, order: &str, loser: Actor, cause: &'static str) {
        if let Some(invoice) = self.bond_invoice_mut(loser, order) {
            invoice.settled = true;
        }
        self.slashed.insert(order.to_owned(), loser);
        self.slash_notice.insert(order.to_owned(), (loser, cause));
        let bond = BOND_POLICY.bond_sats(SATS);
        self.claims.insert(
            order.to_owned(),
            Claim {
                actor: Self::counterparty(loser),
                amount: BOND_POLICY.counterparty_share_sats(bond),
                phase: "pending".into(),
            },
        );
        self.actions.push(format!("daemon:slash-{cause}"));
    }
    /// The daemon's waiting-state deadline for `duty` on `order` elapsed.
    /// Undone, it slashes whoever owed it: a taker's failure puts the order
    /// back on the book, a maker's closes it and returns the escrow. Returns
    /// what the book then shows, or `None` when the duty was done in time.
    pub(crate) fn time_out(&mut self, order: &str, duty: Duty) -> Option<&'static str> {
        let done = match duty {
            Duty::BuyerInvoice => self.buyer_invoice.contains_key(order),
            Duty::Escrow => self.funded.contains(order),
        };
        if done || self.order(order).is_terminal() {
            return None;
        }
        let buyer = self.buyer()?;
        let owes = match duty {
            Duty::BuyerInvoice => buyer,
            Duty::Escrow => Self::counterparty(buyer),
        };
        self.slash(order, owes, "timeout");
        if self.taker() == Some(owes) {
            self.declined.push((owes, order.to_owned()));
            self.set(order, OrderState::Pending);
            return Some("pending");
        }
        if self.funded.contains(order) {
            self.refunded = true;
        }
        self.dropped.push(order.to_owned());
        self.set(order, OrderState::Cancelled);
        Some("canceled")
    }
    /// Whether `actor` walked away from `order`'s bond.
    pub(crate) fn declined(&self, actor: Actor, order: &str) -> bool {
        self.declined
            .iter()
            .any(|(who, id)| *who == actor && id == order)
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
            bonded: false,
            bond_requested: BTreeSet::new(),
            maker_bond_requested: BTreeSet::new(),
            declined: Vec::new(),
            slashed: BTreeMap::new(),
            slash_notice: BTreeMap::new(),
            claims: BTreeMap::new(),
            slashes_on_timeout: false,
            expires_unpaid_bonds: false,
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
                SpendCaps::new(SATS + 2 * BOND_POLICY.bond_sats(SATS), SATS * 10).unwrap(),
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

    /// The daemon asks both sides for the bond [`BOND_POLICY`] sizes, and
    /// the stack declares that policy to the run.
    pub fn bonded(mut self) -> Self {
        self.state.lock().unwrap().bonded = true;
        self.context.limits.bond = Some(BOND_POLICY);
        self
    }
    /// The daemon slashes whoever leaves a waiting step undone once its
    /// deadline elapses: a taker's failure puts the order back on the book,
    /// a maker's closes it.
    pub fn slashes_on_timeout(self) -> Self {
        self.state.lock().unwrap().slashes_on_timeout = true;
        self
    }
    /// The daemon lets a bond nobody pays lapse.
    pub fn expires_unpaid_bonds(self) -> Self {
        self.state.lock().unwrap().expires_unpaid_bonds = true;
        self
    }
    /// The share on offer for `order`, if the daemon offered one.
    pub fn claim(&self, order: &str) -> Option<Claim> {
        self.state.lock().unwrap().claims.get(order).cloned()
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
            .filter(|invoice| invoice.actor == Actor::Bob && !invoice.bond)
            .cloned()
            .collect()
    }
    /// The bond invoices the daemon issued, with what became of each.
    pub fn bond_invoices(&self) -> Vec<IssuedInvoice> {
        self.state
            .lock()
            .unwrap()
            .invoices
            .values()
            .filter(|invoice| invoice.bond)
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

#[path = "flow_fixture_wallet.rs"]
mod wallet;
use wallet::Wallet;
