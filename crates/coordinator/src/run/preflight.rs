//! Read-only desktop preflight, before any actor session or payment is created.
use super::{RunDeps, RunOptions, RunSummary};
use crate::Config;
use mortsom_common::{FailureCategory, check::CheckStatus};
use mortsom_scenario_kit::{
    outcome::{Outcome, ScenarioResult},
    registry::ScenarioDef,
};
use std::time::Duration;

pub(super) async fn check(
    config: &Config,
    options: &RunOptions,
    deps: &RunDeps<'_>,
    summary: &mut RunSummary,
) -> bool {
    if options.stack.is_none() {
        summary.checks.push(
            "desktop-preflight",
            CheckStatus::Fail,
            "public stack snapshot is required",
        );
        return false;
    }
    if let Some(stack) = &options.stack
        && let Err(error) = stack.validate_config(config)
    {
        summary
            .checks
            .push("desktop-preflight", CheckStatus::Fail, error.to_string());
        return false;
    }
    let Some(net) = deps.net else {
        summary.checks.push(
            "desktop-preflight",
            CheckStatus::Fail,
            "read-only network probe is required",
        );
        return false;
    };
    let preflight = crate::doctor::desktop_preflight(config, net).await;
    summary.mostrod_version = preflight.daemon_version;
    summary.checks.checks.extend(preflight.checks.checks);
    if let Some(stack) = &options.stack {
        let (status, detail) = match &preflight.daemon_info {
            Some(info) => bond_check(info, stack.declared_limits.bond.as_ref()),
            None => (
                CheckStatus::Fail,
                "no daemon info event to check the anti-abuse bond against".into(),
            ),
        };
        summary.checks.push("daemon-bond", status, detail);
    }
    summary.checks.is_healthy()
}

/// Whether the daemon answering now runs the bond the stack declared at
/// `stack up`: its info event is what it does, and a daemon restarted with
/// other settings since would leave every scenario reading the wrong policy.
fn bond_check(
    info: &mortsom_environment::daemon::DaemonInfo,
    declared: Option<&crate::stack::DeclaredBond>,
) -> (CheckStatus, String) {
    match crate::stack::verify_advertised_bond(info, declared) {
        Ok(()) if declared.is_some() => (
            CheckStatus::Pass,
            "the daemon advertises the anti-abuse bond the stack declares".into(),
        ),
        Ok(()) => (
            CheckStatus::Pass,
            "the daemon asks for no anti-abuse bond, as the stack declares".into(),
        ),
        Err(error) => (CheckStatus::Fail, error.to_string()),
    }
}

pub(super) fn reject(selected: &[&ScenarioDef], summary: &mut RunSummary) {
    for definition in selected {
        summary.results.push(ScenarioResult {
            name: definition.name.into(),
            app_profile: summary.app_profile.clone(),
            app_version: summary.app_version.clone(),
            attempts: 0,
            duration: Duration::ZERO,
            outcome: Outcome::Failed {
                category: FailureCategory::EnvironmentMisconfiguration,
                message: "desktop preflight failed before client startup".into(),
            },
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mortsom_environment::daemon::DaemonInfo;

    fn info(enabled: &str) -> DaemonInfo {
        DaemonInfo {
            pubkey: "daemon".into(),
            created_at: 0,
            tags: [("bond_enabled".to_owned(), enabled.to_owned())]
                .into_iter()
                .collect(),
        }
    }

    #[test]
    fn a_daemon_asking_for_bonds_on_a_bondless_stack_fails_the_preflight() {
        assert_eq!(bond_check(&info("false"), None).0, CheckStatus::Pass);
        let (status, detail) = bond_check(&info("true"), None);
        assert_eq!(status, CheckStatus::Fail);
        assert!(detail.contains("--bond"), "{detail}");
    }

    #[test]
    fn a_bonded_stack_whose_daemon_dropped_the_bond_fails_the_preflight() {
        let declared = crate::stack::DeclaredBond {
            apply_to: "both".into(),
            amount_bps: 100,
            base_amount_sats: 1000,
            slash_on_waiting_timeout: true,
            node_share_bps: 5000,
            payout_claim_window_days: 1,
        };
        assert_eq!(
            bond_check(&info("false"), Some(&declared)).0,
            CheckStatus::Fail
        );
    }
}
