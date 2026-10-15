//! A scenario that fails because the stack went away under it is reported
//! as the infrastructure's failure, not the app's. One that only spent the
//! wallets below the pre-run floor keeps its own category: the stack still
//! answers, and the floor is a guard for starting, not a sign of life.
use std::sync::atomic::{AtomicBool, Ordering};

use super::*;

/// Set by [`stack_dies`]; only the test running it reads it.
static STACK_DOWN: AtomicBool = AtomicBool::new(false);
/// Set by [`wallets_drain`]; only the test running it reads it.
static WALLETS_DRAINED: AtomicBool = AtomicBool::new(false);

/// What the scenario leaves behind.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Change {
    /// Nothing listens and no relay connects.
    Down,
    /// Everything answers; the wallets hold nothing.
    Drained,
}

/// [`ReadyNet`] until `flag` is set, then the stack as `change` leaves it.
struct StackAfter {
    flag: &'static AtomicBool,
    change: Change,
}

impl StackAfter {
    fn is(&self, change: Change) -> bool {
        self.change == change && self.flag.load(Ordering::SeqCst)
    }
}

#[async_trait]
impl NetProbes for StackAfter {
    async fn local_port(&self, port: u16) -> PortState {
        if self.is(Change::Down) {
            PortState::Free
        } else {
            ReadyNet.local_port(port).await
        }
    }

    async fn daemon(&self, relays: &[String], pubkey: PublicKey) -> DaemonProbe {
        let probe = ReadyNet.daemon(relays, pubkey).await;
        if !self.is(Change::Down) {
            return probe;
        }
        DaemonProbe {
            relays: probe
                .relays
                .into_iter()
                .map(|relay| RelayProbe {
                    error: Some("connection refused".into()),
                    ..relay
                })
                .collect(),
            info: None,
            relay_list: None,
        }
    }

    async fn wallet(
        &self,
        label: &str,
        wallet: &WalletConfig,
    ) -> Result<WalletProbe, WalletProbeError> {
        let probe = ReadyNet.wallet(label, wallet).await?;
        if !self.is(Change::Drained) {
            return Ok(probe);
        }
        Ok(WalletProbe {
            balance_sats: 0,
            inbound_sats: Some(0),
            ..probe
        })
    }
}

/// The apps lose the stack mid-scenario and report what they saw next.
fn stack_dies(_ctx: &Context) -> ScenarioFuture<'_> {
    Box::pin(async {
        STACK_DOWN.store(true, Ordering::SeqCst);
        Err(mortsom_scenario_kit::context::ScenarioError::new(
            mortsom_common::FailureCategory::Product,
            "no new trade appeared in the trades list after creating the order",
        ))
    })
}

/// The trade spent the wallets, then the app got something wrong.
fn wallets_drain(_ctx: &Context) -> ScenarioFuture<'_> {
    Box::pin(async {
        WALLETS_DRAINED.store(true, Ordering::SeqCst);
        Err(mortsom_scenario_kit::context::ScenarioError::new(
            mortsom_common::FailureCategory::Product,
            "the buyer never saw the payout",
        ))
    })
}

inventory::submit! {
    ScenarioDef {
        name: "zz_runner_stack_dies",
        tags: &["runner-selftest"],
        requires: &[],
        timeout_secs: 5,
        moves_value: false,
        needs_app_wallet: false,
        needs_solver: false,
        needs: &[],
        run: stack_dies,
    }
}

inventory::submit! {
    ScenarioDef {
        name: "zz_runner_wallets_drain",
        tags: &["runner-selftest"],
        requires: &[],
        timeout_secs: 5,
        moves_value: false,
        needs_app_wallet: false,
        needs_solver: false,
        needs: &[],
        run: wallets_drain,
    }
}

/// Runs `scenario` against `net` and returns its category and message.
async fn failure_of(scenario: &str, net: &StackAfter) -> (mortsom_common::FailureCategory, String) {
    net.flag.store(false, Ordering::SeqCst);
    let world = world(false);
    let summary = run_on(
        &world,
        Arc::new(FakeAdapter::full()),
        opts_in(&world, &[scenario]),
        false,
        &FakeObserver::default(),
        "regtest",
        net,
    )
    .await
    .unwrap();
    let Outcome::Failed { category, message } = &summary.results[0].outcome else {
        panic!("{:?}", summary.results);
    };
    (*category, message.clone())
}

#[tokio::test]
async fn a_failure_after_the_stack_went_away_is_the_infrastructures() {
    // Arrange
    let net = StackAfter {
        flag: &STACK_DOWN,
        change: Change::Down,
    };

    // Act
    let (category, message) = failure_of("zz_runner_stack_dies", &net).await;

    // Assert
    assert_eq!(
        category,
        mortsom_common::FailureCategory::TestInfrastructure
    );
    assert!(message.contains("the stack stopped answering"), "{message}");
    assert!(message.contains("relay-port"), "{message}");
    assert!(
        message.contains("the scenario reported product: no new trade appeared in the trades list"),
        "{message}"
    );
}

#[tokio::test]
async fn a_failure_that_only_spent_the_wallets_keeps_its_own_category() {
    // Arrange
    let net = StackAfter {
        flag: &WALLETS_DRAINED,
        change: Change::Drained,
    };

    // Act
    let (category, message) = failure_of("zz_runner_wallets_drain", &net).await;

    // Assert
    assert_eq!(category, mortsom_common::FailureCategory::Product);
    assert_eq!(message, "the buyer never saw the payout");
}
