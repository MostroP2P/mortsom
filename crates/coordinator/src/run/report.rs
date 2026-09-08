//! Scenario run report schema and final verdict rendering.
use crate::{
    artifacts::{RunArtifacts, files},
    bootstrap::ActorSetup,
    report::{RunReport, write_report},
};
use mortsom_android_driver::farm::TeardownItem;
use mortsom_common::check::DoctorReport;
use mortsom_common::{Lane, RunId};
use mortsom_environment::relay::{RelayHandle, RelayTeardown};
use mortsom_scenario_kit::{
    budget::Totals,
    cleanup::CleanupReport,
    outcome::{Outcome, ScenarioResult},
};
use serde::{Deserialize, Serialize};

/// Machine-readable outcome, written to `summary.json`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunSummary {
    /// Run identifier.
    pub run_id: RunId,
    /// Lane.
    pub lane: Lane,
    /// Command.
    pub command: String,
    /// App profile.
    pub app_profile: String,
    /// App version, when known.
    pub app_version: Option<String>,
    /// Version observed in a fresh signed daemon info event.
    #[serde(default)]
    pub mostrod_version: Option<String>,
    /// Public persistent stack snapshot, absent in historical Android reports.
    #[serde(default)]
    pub stack: Option<crate::stack::StackSnapshot>,
    /// Selected client platform; absent in historical reports.
    #[serde(default)]
    pub platform: Option<mortsom_desktop_driver::Platform>,
    /// Full client bundle content identity, absent for historical reports.
    #[serde(default)]
    pub app_artifact: Option<crate::fingerprint::BundleFingerprint>,
    /// Whether mainnet clearance was granted.
    pub mainnet_cleared: bool,
    /// Relay used.
    pub relay: Option<RelayHandle>,
    /// Actor bootstrap outcomes.
    pub actors: Vec<ActorSetup>,
    /// Scenario results.
    pub results: Vec<ScenarioResult>,
    /// Environment and bootstrap checks.
    pub checks: DoctorReport,
    /// Cleanup report.
    pub cleanup: Option<CleanupReport>,
    /// Budget totals.
    pub budget: Option<Totals>,
    /// Farm teardown.
    pub farm_teardown: Vec<TeardownItem>,
    /// Relay teardown.
    pub relay_teardown: Option<RelayTeardown>,
    /// True when every check passed and every selected scenario passed or
    /// was skipped with a reason.
    pub passed: bool,
}

/// Persists cleanup routing before any client can start, including interrupted bootstrap.
pub(super) fn record_platform(
    artifacts: &RunArtifacts,
    summary: &RunSummary,
) -> Result<(), String> {
    artifacts
        .write_json(
            files::ENVIRONMENT,
            &serde_json::json!({
                "run_id": summary.run_id, "platform": summary.platform, "status": "starting",
            }),
        )
        .map_err(|error| format!("cannot record run platform before startup: {error}"))
}

pub(super) fn finish(artifacts: &RunArtifacts, summary: &mut RunSummary) -> Result<(), String> {
    let scenarios_ok = summary
        .results
        .iter()
        .all(|r| !matches!(r.outcome, Outcome::Failed { .. }));
    let clean = summary.cleanup.as_ref().is_none_or(CleanupReport::is_clean);
    summary.passed = summary.checks.is_healthy() && scenarios_ok && clean;
    let cleanup = serde_json::json!({
        "orders": summary.cleanup,
        "farm": summary.farm_teardown,
        "relay": summary.relay_teardown,
    });
    let environment = serde_json::json!({
        "run_id": summary.run_id,
        "lane": summary.lane,
        "command": summary.command,
        "app_profile": summary.app_profile,
        "app_version": summary.app_version,
        "mostrod_version": summary.mostrod_version,
        "platform": summary.platform,
        "stack": summary.stack,
        "app_artifact": summary.app_artifact,
        "mainnet_cleared": summary.mainnet_cleared,
        "relay": summary.relay.as_ref().map(|r| serde_json::json!({"container_name": r.container_name, "host_port": r.host_port})),
    });
    // The timeline is the scenario timeline written at teardown; the report
    // writer's table would overwrite it, so a run keeps its own file and the
    // report only writes summary/environment/cleanup.
    let report = RunReport {
        stack: summary.stack.as_ref(),
        app_artifact: summary.app_artifact.as_ref(),
        run_id: &summary.run_id,
        lane: summary.lane,
        command: &summary.command,
        app_profile: Some(&summary.app_profile),
        checks: &summary.checks,
        passed: summary.passed,
    };
    let timeline_exists = artifacts
        .file(files::TIMELINE)
        .map(|p| p.is_file())
        .unwrap_or(false);
    write_report(
        artifacts,
        &report,
        summary,
        &environment,
        &cleanup,
        !timeline_exists,
    )
}
