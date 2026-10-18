//! A run never starts against a daemon in maintenance mode: it would refuse
//! every order the scenario creates, and the failure would read as the
//! app's.
use super::*;

/// [`ReadyNet`], with the daemon's info event carrying `maintenance_mode`
/// set to the value held.
pub(super) struct Advertising(pub(super) &'static str);

#[async_trait]
impl NetProbes for Advertising {
    async fn local_port(&self, port: u16) -> PortState {
        ReadyNet.local_port(port).await
    }

    async fn daemon(&self, relays: &[String], pubkey: PublicKey) -> DaemonProbe {
        let probe = ReadyNet.daemon(relays, pubkey).await;
        let info = probe.info.map(|mut info| {
            info.tags
                .insert("maintenance_mode".to_owned(), self.0.to_owned());
            info
        });
        DaemonProbe { info, ..probe }
    }

    async fn wallet(
        &self,
        label: &str,
        wallet: &WalletConfig,
    ) -> Result<WalletProbe, WalletProbeError> {
        ReadyNet.wallet(label, wallet).await
    }
}

async fn run_against(world: &World, net: &dyn NetProbes) -> RunSummary {
    run_on(
        world,
        Arc::new(FakeAdapter::full()),
        opts_in(world, &["happy_sell"]),
        false,
        &FakeObserver::default(),
        "regtest",
        net,
    )
    .await
    .unwrap()
}

fn maintenance_check(summary: &RunSummary) -> &mortsom_common::check::CheckResult {
    summary
        .checks
        .checks
        .iter()
        .find(|check| check.name == "daemon-maintenance")
        .unwrap_or_else(|| panic!("no daemon-maintenance check:\n{}", summary.checks.render()))
}

#[tokio::test]
async fn a_daemon_in_maintenance_mode_stops_the_run_before_any_client_starts() {
    // Arrange
    mortsom_scenarios::link();
    let world = world(false);

    // Act
    let summary = run_against(&world, &Advertising("true")).await;

    // Assert
    assert!(!summary.passed);
    assert!(summary.actors.is_empty() && summary.farm_teardown.is_empty());
    let check = maintenance_check(&summary);
    assert_eq!(check.status, CheckStatus::Fail);
    assert!(check.detail.contains("stale chain tip"), "{}", check.detail);
    let [result] = summary.results.as_slice() else {
        panic!("{:?}", summary.results);
    };
    assert_eq!(result.attempts, 0);
    assert!(
        matches!(
            result.outcome,
            Outcome::Failed {
                category: mortsom_common::FailureCategory::EnvironmentMisconfiguration,
                ..
            }
        ),
        "{:?}",
        result.outcome
    );
}

#[tokio::test]
async fn a_daemon_out_of_maintenance_mode_lets_the_run_start() {
    // Arrange
    mortsom_scenarios::link();
    let world = world(false);

    // Act
    let summary = run_against(&world, &Advertising("false")).await;

    // Assert
    assert_eq!(maintenance_check(&summary).status, CheckStatus::Pass);
    assert!(
        summary.passed,
        "{}\n{:?}",
        summary.checks.render(),
        summary.results
    );
}
