//! A scenario that fails because the stack went away under it is reported
//! as the infrastructure's failure, not the app's. One that only spent the
//! wallets below the pre-run floor keeps its own category: the stack still
//! answers, and the floor is a guard for starting, not a sign of life. One
//! that fails while the daemon answers in maintenance mode is the
//! environment's.
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use super::*;

/// One flag per test, each set by that test's scenario alone: the tests run
/// in parallel and must not see each other's stack change.
static STACK_DOWN: AtomicBool = AtomicBool::new(false);
static WALLETS_DRAINED: AtomicBool = AtomicBool::new(false);
static DAEMON_SILENT: AtomicBool = AtomicBool::new(false);
static RELAY_BLIPS: AtomicBool = AtomicBool::new(false);
static DAEMON_DRAINS: AtomicBool = AtomicBool::new(false);

/// What the scenario leaves behind.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Change {
    /// Nothing listens and no relay connects.
    Down,
    /// Everything answers; the wallets hold nothing.
    Drained,
    /// The relay answers, still serving the daemon's last info event,
    /// published ten minutes ago: the daemon is gone.
    DaemonSilent,
    /// The relay's port refuses one connection, then answers again.
    Blip,
    /// Everything answers; the daemon advertises maintenance mode.
    Drains,
}

/// [`ReadyNet`] until `flag` is set, then the stack as `change` leaves it.
struct StackAfter {
    flag: &'static AtomicBool,
    change: Change,
    /// Port probes taken since the change.
    port_probes: AtomicUsize,
}

impl StackAfter {
    fn new(flag: &'static AtomicBool, change: Change) -> Self {
        flag.store(false, Ordering::SeqCst);
        Self {
            flag,
            change,
            port_probes: AtomicUsize::new(0),
        }
    }

    fn is(&self, change: Change) -> bool {
        self.change == change && self.flag.load(Ordering::SeqCst)
    }
}

#[async_trait]
impl NetProbes for StackAfter {
    async fn local_port(&self, port: u16) -> PortState {
        let refused = self.is(Change::Down)
            || (self.is(Change::Blip) && self.port_probes.fetch_add(1, Ordering::SeqCst) == 0);
        if refused {
            PortState::Free
        } else {
            ReadyNet.local_port(port).await
        }
    }

    async fn daemon(&self, relays: &[String], pubkey: PublicKey) -> DaemonProbe {
        if self.is(Change::Drains) {
            return super::maintenance::Advertising("true")
                .daemon(relays, pubkey)
                .await;
        }
        let probe = ReadyNet.daemon(relays, pubkey).await;
        if self.is(Change::DaemonSilent) {
            let info = probe.info.map(|info| DaemonInfo {
                created_at: info.created_at - 600,
                ..info
            });
            return DaemonProbe { info, ..probe };
        }
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

/// Sets `flag`, as the stack changing under the apps, and fails as a
/// product failure, as the apps would report it.
fn change_then_fail(flag: &'static AtomicBool, message: &'static str) -> ScenarioFuture<'static> {
    Box::pin(async move {
        flag.store(true, Ordering::SeqCst);
        Err(mortsom_scenario_kit::context::ScenarioError::new(
            mortsom_common::FailureCategory::Product,
            message,
        ))
    })
}

const NO_TRADE: &str = "no new trade appeared in the trades list after creating the order";
const NO_PAYOUT: &str = "the buyer never saw the payout";

fn stack_dies(_ctx: &Context) -> ScenarioFuture<'_> {
    change_then_fail(&STACK_DOWN, NO_TRADE)
}
fn wallets_drain(_ctx: &Context) -> ScenarioFuture<'_> {
    change_then_fail(&WALLETS_DRAINED, NO_PAYOUT)
}
fn daemon_dies(_ctx: &Context) -> ScenarioFuture<'_> {
    change_then_fail(&DAEMON_SILENT, NO_TRADE)
}
fn relay_blips(_ctx: &Context) -> ScenarioFuture<'_> {
    change_then_fail(&RELAY_BLIPS, NO_PAYOUT)
}
fn daemon_drains(_ctx: &Context) -> ScenarioFuture<'_> {
    change_then_fail(&DAEMON_DRAINS, NO_TRADE)
}

macro_rules! selftest {
    ($name:literal, $body:ident) => {
        inventory::submit! {
            ScenarioDef {
                name: $name,
                tags: &["runner-selftest"],
                requires: &[],
                timeout_secs: 5,
                moves_value: false,
                needs_app_wallet: false,
                needs_solver: false,
                needs: &[],
                run: $body,
            }
        }
    };
}
selftest!("zz_runner_stack_dies", stack_dies);
selftest!("zz_runner_wallets_drain", wallets_drain);
selftest!("zz_runner_daemon_dies", daemon_dies);
selftest!("zz_runner_relay_blips", relay_blips);
selftest!("zz_runner_daemon_drains", daemon_drains);

/// How a scenario's failure was reported.
struct Reported {
    category: mortsom_common::FailureCategory,
    message: String,
    timeline: String,
    /// The run's `run.log`, empty when the run kept none.
    log: String,
}

/// Runs `scenario` against `net`, on a stack whose daemon declares it
/// publishes its info event every 30 seconds.
async fn failure_of(scenario: &str, net: &StackAfter) -> Reported {
    let mut world = world(false);
    let settings = world.dir.path().join("mostro-settings.toml");
    std::fs::write(&settings, "[mostro]\npublish_mostro_info_interval = 30\n").unwrap();
    world.config.mostro.settings_file = settings.display().to_string();
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
    let run_dir = world
        .dir
        .path()
        .join("artifacts")
        .join(summary.run_id.as_str());
    Reported {
        category: *category,
        message: message.clone(),
        timeline: std::fs::read_to_string(run_dir.join(files::TIMELINE)).unwrap(),
        log: std::fs::read_to_string(run_dir.join("run.log")).unwrap_or_default(),
    }
}

#[tokio::test]
async fn a_failure_after_the_stack_went_away_is_the_infrastructures() {
    // Arrange
    let net = StackAfter::new(&STACK_DOWN, Change::Down);

    // Act
    let reported = failure_of("zz_runner_stack_dies", &net).await;

    // Assert
    let message = &reported.message;
    assert_eq!(
        reported.category,
        mortsom_common::FailureCategory::TestInfrastructure
    );
    assert!(message.contains("the stack stopped answering"), "{message}");
    assert!(message.contains("relay-port"), "{message}");
    assert!(
        message.contains(&format!("the scenario reported product: {NO_TRADE}")),
        "{message}"
    );
    assert!(
        reported.timeline.contains("the stack stopped answering"),
        "{}",
        reported.timeline
    );
}

#[tokio::test]
async fn a_failure_after_the_daemon_went_silent_is_the_infrastructures() {
    // Arrange
    let net = StackAfter::new(&DAEMON_SILENT, Change::DaemonSilent);

    // Act
    let reported = failure_of("zz_runner_daemon_dies", &net).await;

    // Assert
    let message = &reported.message;
    assert_eq!(
        reported.category,
        mortsom_common::FailureCategory::TestInfrastructure
    );
    assert!(message.contains("mostro-daemon"), "{message}");
    assert!(!message.contains("relay-port"), "{message}");
}

#[tokio::test]
async fn a_failure_that_only_spent_the_wallets_keeps_its_own_category() {
    // Arrange
    let net = StackAfter::new(&WALLETS_DRAINED, Change::Drained);

    // Act
    let reported = failure_of("zz_runner_wallets_drain", &net).await;

    // Assert
    assert_eq!(reported.category, mortsom_common::FailureCategory::Product);
    assert_eq!(reported.message, NO_PAYOUT);
}

#[tokio::test]
async fn a_probe_that_fails_once_attributes_nothing_and_says_nothing() {
    // Arrange
    let net = StackAfter::new(&RELAY_BLIPS, Change::Blip);

    // Act
    let reported = failure_of("zz_runner_relay_blips", &net).await;

    // Assert
    assert_eq!(reported.category, mortsom_common::FailureCategory::Product);
    assert_eq!(reported.message, NO_PAYOUT);
    // Only the attempt's own line: no attribution recorded beside it.
    assert_eq!(
        reported.timeline.matches(NO_PAYOUT).count(),
        1,
        "{}",
        reported.timeline
    );
    assert!(
        !reported.log.contains("attributed to the stack"),
        "{}",
        reported.log
    );
}

#[tokio::test]
async fn a_failure_while_the_daemon_drains_is_the_environments() {
    // Arrange
    let net = StackAfter::new(&DAEMON_DRAINS, Change::Drains);

    // Act
    let reported = failure_of("zz_runner_daemon_drains", &net).await;

    // Assert
    let message = &reported.message;
    assert_eq!(
        reported.category,
        mortsom_common::FailureCategory::EnvironmentMisconfiguration
    );
    assert!(message.contains("maintenance_mode=true"), "{message}");
    assert!(
        message.contains(&format!("the scenario reported product: {NO_TRADE}")),
        "{message}"
    );
    assert!(
        reported.timeline.contains("maintenance_mode=true"),
        "{}",
        reported.timeline
    );
}
