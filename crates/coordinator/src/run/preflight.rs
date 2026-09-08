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
    if options.desktop.is_none() {
        return true;
    }
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
    summary.checks.is_healthy()
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
