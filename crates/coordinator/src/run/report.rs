//! Scenario run report schema and final verdict rendering.
use super::farm::TeardownItem;
use crate::{
    artifacts::{RunArtifacts, files},
    bootstrap::ActorSetup,
    report::{RunReport, write_report},
};
use mortsom_common::check::DoctorReport;
use mortsom_common::{Lane, RunId};
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
    /// Public persistent stack snapshot, absent in historical reports.
    #[serde(default)]
    pub stack: Option<crate::stack::StackSnapshot>,
    /// Stable identity of the public stack configuration, when available.
    #[serde(default)]
    pub stack_fingerprint: Option<crate::stack::StackFingerprint>,
    /// Selected client platform; absent in historical reports.
    #[serde(default)]
    pub platform: Option<mortsom_desktop_driver::Platform>,
    /// Full client bundle content identity, absent for historical reports.
    #[serde(default)]
    pub app_artifact: Option<crate::fingerprint::BundleFingerprint>,
    /// Source/build history; supplied artifacts explicitly report unknown history.
    #[serde(default)]
    pub app_build: Option<crate::provenance::BuildProvenance>,
    /// Whether mainnet clearance was granted.
    pub mainnet_cleared: bool,
    /// Actor bootstrap outcomes.
    pub actors: Vec<ActorSetup>,
    /// Scenario results.
    pub results: Vec<ScenarioResult>,
    /// Environment and bootstrap checks.
    pub checks: DoctorReport,
    /// Cleanup report.
    pub cleanup: Option<CleanupReport>,
    /// Durable identity retention and the original recovery run.
    #[serde(default)]
    pub recovery: Option<RecoveryStatus>,
    /// Budget totals.
    pub budget: Option<Totals>,
    /// Farm teardown.
    pub farm_teardown: Vec<TeardownItem>,
    /// True when every check passed and every selected scenario passed or
    /// was skipped with a reason.
    pub passed: bool,
}

/// Public recovery lifecycle evidence, without filesystem or credential contents.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecoveryStatus {
    /// Run whose identities and orders are owned by the journal.
    pub original_run_id: RunId,
    /// Whether protected client profiles remain available for cleanup.
    pub retained: bool,
    /// Whether order and wallet resolution was durably verified.
    pub resolution_verified: bool,
}

/// Persists public build/stack facts and cleanup routing before clients can start.
pub(super) fn record_environment(
    artifacts: &RunArtifacts,
    summary: &RunSummary,
) -> Result<(), String> {
    artifacts
        .write_metadata_json(
            files::ENVIRONMENT,
            &environment(summary, "starting"),
            summary.stack.as_ref(),
            summary.app_artifact.as_ref(),
            summary.app_build.as_ref(),
        )
        .map_err(|error| format!("cannot record environment before startup: {error}"))
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
        "recovery": summary.recovery,
        "farm": summary.farm_teardown,
    });
    let environment = environment(summary, "finished");
    // The timeline is the scenario timeline written at teardown; the report
    // writer's table would overwrite it, so a run keeps its own file and the
    // report only writes summary/environment/cleanup.
    let report = RunReport {
        stack: summary.stack.as_ref(),
        app_artifact: summary.app_artifact.as_ref(),
        app_build: summary.app_build.as_ref(),
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

/// Builds the report before any client starts.
pub(super) fn initial_summary(
    config: &crate::Config,
    options: &super::RunOptions,
    run_id: RunId,
    app_profile: String,
    mainnet_cleared: bool,
) -> Result<RunSummary, String> {
    Ok(RunSummary {
        run_id,
        lane: config.run.lane,
        command: "run".into(),
        app_profile,
        app_version: None,
        app_build: Some(options.desktop.provenance.clone()),
        mostrod_version: None,
        stack: options.stack.clone(),
        stack_fingerprint: options
            .stack
            .as_ref()
            .map(crate::stack::StackSnapshot::fingerprint)
            .transpose()?,
        app_artifact: Some(options.desktop.bundle.fingerprint().clone()),
        platform: Some(options.desktop.platform),
        mainnet_cleared,
        actors: Vec::new(),
        results: Vec::new(),
        checks: DoctorReport::default(),
        cleanup: None,
        recovery: None,
        budget: None,
        farm_teardown: Vec::new(),
        passed: false,
    })
}

fn environment(summary: &RunSummary, status: &str) -> serde_json::Value {
    serde_json::json!({
        "run_id": summary.run_id,
        "lane": summary.lane,
        "command": summary.command,
        "app_profile": summary.app_profile,
        "app_version": summary.app_version,
        "mostrod_version": summary.mostrod_version,
        "platform": summary.platform,
        "stack": summary.stack,
        "stack_fingerprint": summary.stack_fingerprint,
        "status": status,
        "app_artifact": summary.app_artifact,
        "app_build": summary.app_build,
        "mainnet_cleared": summary.mainnet_cleared,
    })
}
