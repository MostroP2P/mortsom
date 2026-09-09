//! In-memory UI, daemon-event and wallet boundaries for buy-order sequencing.
use super::super::*;
use async_trait::async_trait;
use mortsom_android_driver::ui::tests_support::ScriptedUi;
use mortsom_common::{Lane, Secret};
use mortsom_observers::{
    relay::{ObserverScope, RelayObserver},
    wallet::{PaymentState, WalletObserver, WalletReader},
};
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

const ORDER: &str = "buy-order";
// Public BOLT 11 reference invoice, never submitted to a real wallet.
const INVOICE: &str = "lnbc2500u1pvjluezsp5zyg3zyg3zyg3zyg3zyg3zyg3zyg3zyg3zyg3zyg3zyg3zyg3zygspp5qqqsyqcyq5rqwzqfqqqsyqcyq5rqwzqfqqqsyqcyq5rqwzqfqypqdq5xysxxatsyp3k7enxv4jsxqzpu9qrsgquk0rl77nj30yxdy8j9vdx85fkpmdla2087ne0xh8nhedh8w27kyke0lp53ut353s06fv3qfegext0eh0ymjpf39tuven09sam30g4vgpfna3rh";

struct State {
    order: OrderState,
    created: Option<(Actor, OrderKind)>,
    actions: Vec<String>,
    wallet_checks: Vec<String>,
    hold_started: bool,
    fail_payment: bool,
}

pub struct Fixture {
    pub context: Context,
    pub adapter: Arc<Adapter>,
    state: Arc<Mutex<State>>,
}

impl Fixture {
    pub async fn new(fail_payment: bool) -> Self {
        let keys = Keys::generate();
        let mut scope = ObserverScope::daemon_only(keys.public_key());
        scope.add_order(ORDER);
        let relay = Arc::new(RelayObserver::detached(scope));
        let state = Arc::new(Mutex::new(State {
            order: OrderState::Pending,
            created: None,
            actions: Vec::new(),
            wallet_checks: Vec::new(),
            hold_started: false,
            fail_payment,
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
            state_timeout: Duration::from_millis(50),
            settle_timeout: Duration::from_millis(50),
            expiration_timeout: Duration::from_millis(50),
            recovery: RecoveryTiming {
                quiet_window: Duration::ZERO,
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
    pub fn actions(&self) -> Vec<String> {
        self.state.lock().unwrap().actions.clone()
    }
    pub fn wallet_checks(&self) -> Vec<String> {
        self.state.lock().unwrap().wallet_checks.clone()
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
        assert!(
            self.state.lock().unwrap().hold_started,
            "buyer invoice must follow accepted escrow"
        );
        Ok(Secret::new(INVOICE))
    }
    async fn start_payment(&self, _: &Secret) -> Result<(), String> {
        assert_eq!(self.actor, Actor::Alice);
        let mut state = self.state.lock().unwrap();
        state.actions.push("pay:alice".into());
        if state.fail_payment {
            return Err("injected hold payment failure".into());
        }
        state.hold_started = true;
        Ok(())
    }
}
#[async_trait]
impl WalletReader for Wallet {
    async fn lookup(&self, _: &str) -> Result<Option<(u64, u64, PaymentState)>, String> {
        let mut state = self.state.lock().unwrap();
        let settled = state.order == OrderState::Completed;
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
