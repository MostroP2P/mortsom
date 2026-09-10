//! In-memory UI, daemon-event and wallet boundaries for scenario
//! sequencing: a daemon that publishes what the real one publishes, two
//! silent actors, and wallets that answer for the payments the daemon
//! made or returned.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use async_trait::async_trait;
use mortsom_android_driver::avd::Actor;
use mortsom_android_driver::ui::tests_support::ScriptedUi;
use mortsom_app_contract::{FiatAmount, OrderDraft, OrderKind, OrderPrice, OrderState};
use mortsom_common::{Lane, Secret};
use mortsom_observers::{
    relay::{ObserverScope, RelayObserver},
    wallet::{PaymentState, WalletObserver, WalletReader},
};
use mortsom_scenario_kit::context::Context;
use mortsom_scenario_kit::{
    budget::{Budget, SpendCaps},
    cleanup::CleanupRegistry,
    context::{ActorSession, MainnetClearance, NodeWallet, RecoveryTiming},
    evidence::NullSink,
    recovery::NoRelayControl,
    timeline::Timeline,
};
use nostr_sdk::prelude::{EventBuilder, FinalizeEvent, Keys, Kind, Tag};
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
    time::Duration,
};

#[path = "flow_test_adapter.rs"]
mod adapter;
pub use adapter::Adapter;

/// The order every scenario creates.
pub const ORDER: &str = "the-order";
/// The remainder the daemon spawns from a range order.
pub const REMAINDER: &str = "the-remainder";
// Public BOLT 11 reference invoice, never submitted to a real wallet.
const INVOICE: &str = "lnbc2500u1pvjluezsp5zyg3zyg3zyg3zyg3zyg3zyg3zyg3zyg3zyg3zyg3zyg3zyg3zygspp5qqqsyqcyq5rqwzqfqqqsyqcyq5rqwzqfqqqsyqcyq5rqwzqfqypqdq5xysxxatsyp3k7enxv4jsxqzpu9qrsgquk0rl77nj30yxdy8j9vdx85fkpmdla2087ne0xh8nhedh8w27kyke0lp53ut353s06fv3qfegext0eh0ymjpf39tuven09sam30g4vgpfna3rh";

/// How the fixture daemon deviates from the well-behaved one.
#[derive(Debug, Clone, Copy, Default)]
pub struct Misbehaviour {
    /// The seller's hold payment fails.
    pub fail_payment: bool,
    /// A lone cancel request on a funded order closes it outright.
    pub lone_cancel_closes: bool,
    /// A market order is published with a sats amount.
    pub prices_market_orders_upfront: bool,
}

struct State {
    order: OrderState,
    created: Option<(Actor, OrderKind)>,
    draft: Option<OrderDraft>,
    actions: Vec<String>,
    wallet_checks: Vec<String>,
    hold_started: bool,
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
    misbehaviour: Misbehaviour,
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
            order: OrderState::Pending,
            created: None,
            draft: None,
            actions: Vec::new(),
            wallet_checks: Vec::new(),
            hold_started: false,
            refunded: false,
            taken_at: None,
            remainder: None,
            cancel_requests: Vec::new(),
            rated: Vec::new(),
            reaps_abandoned_take_to: None,
            expires_pending: false,
            cancels_funded_take: false,
            dropped: Vec::new(),
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
            budget: Arc::new(Mutex::new(Budget::new(
                SpendCaps::new(250_000, 250_000).unwrap(),
            ))),
            cleanup: Arc::new(CleanupRegistry::default()),
            timeline: Arc::new(Timeline::default()),
            evidence: Arc::new(NullSink),
            journal: Arc::new(mortsom_scenario_kit::journal::NoJournal),
            pending_holds: Mutex::new(BTreeMap::new()),
            mainnet: Some(MainnetClearance { lane: Lane::Local }),
            amount_sats: 250_000,
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

    pub fn actions(&self) -> Vec<String> {
        self.state.lock().unwrap().actions.clone()
    }
    pub fn wallet_checks(&self) -> Vec<String> {
        self.state.lock().unwrap().wallet_checks.clone()
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
#[async_trait]
impl NodeWallet for Wallet {
    async fn make_invoice(&self, sats: u64, _: &str) -> Result<Secret, String> {
        assert_eq!(self.actor, Actor::Bob);
        assert_eq!(sats, 250_000);
        let state = self.state.lock().unwrap();
        // The maker of a buy is asked for the invoice only once the seller's
        // hold is accepted; the taker of a sell hands it in first.
        if matches!(state.created, Some((_, OrderKind::Buy))) {
            assert!(
                state.hold_started,
                "buyer invoice must follow accepted escrow"
            );
        }
        Ok(Secret::new(INVOICE))
    }
    async fn start_payment(&self, _: &Secret) -> Result<(), String> {
        assert_eq!(self.actor, Actor::Alice);
        let mut state = self.state.lock().unwrap();
        state.actions.push("pay:alice".into());
        if state.misbehaviour.fail_payment {
            return Err("injected hold payment failure".into());
        }
        state.hold_started = true;
        // On a sell the daemon tells both parties `active` once the hold
        // is accepted; on a buy it first asks the maker for the invoice.
        if matches!(state.created, Some((_, OrderKind::Sell))) {
            state.order = OrderState::Active;
        }
        Ok(())
    }
}
#[async_trait]
impl WalletReader for Wallet {
    async fn lookup(&self, _: &str) -> Result<Option<(u64, u64, PaymentState)>, String> {
        let mut state = self.state.lock().unwrap();
        let settled = state.order == OrderState::Completed;
        if state.refunded {
            assert_eq!(self.actor, Actor::Alice);
            state.wallet_checks.push("alice:failed".into());
            return Ok(Some((250_000, 0, PaymentState::Failed)));
        }
        if !settled {
            assert_eq!(self.actor, Actor::Alice);
            assert!(state.hold_started);
        }
        state.wallet_checks.push(format!(
            "{}:{}",
            self.actor,
            if settled { "settled" } else { "pending" }
        ));
        Ok(Some((
            250_000,
            0,
            if settled {
                PaymentState::Settled
            } else {
                PaymentState::Pending
            },
        )))
    }
}
