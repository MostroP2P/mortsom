//! Shared run report writing: `summary.json`, `environment.json`,
//! `cleanup.json` and `timeline.md` in the spec §11 layout. Every command
//! that owns a run uses this so the artifacts never drift apart.

use mortsom_common::check::DoctorReport;
use mortsom_common::{Lane, RunId};
use serde::Serialize;
use tracing::info;

use crate::artifacts::{RunArtifacts, files};

/// What every command reports about itself.
pub struct RunReport<'a> {
    /// Run identifier.
    pub run_id: &'a RunId,
    /// Lane.
    pub lane: Lane,
    /// Command name (`relay smoke`, `device smoke`, `app setup`).
    pub command: &'a str,
    /// App profile when an app was involved.
    pub app_profile: Option<&'a str>,
    /// Checks in order.
    pub checks: &'a DoctorReport,
    /// Whether the command passed.
    pub passed: bool,
    /// Verified public metadata whose hashes must remain reproducible.
    pub stack: Option<&'a crate::stack::StackSnapshot>,
    /// Complete client bundle content identity.
    pub app_artifact: Option<&'a crate::fingerprint::BundleFingerprint>,
}

/// Writes the report artifacts. `summary` is the command's own
/// machine-readable summary; `environment` and `cleanup` are free-form.
/// `write_timeline` is false for commands that render their own timeline.
pub fn write_report<S: Serialize, E: Serialize, C: Serialize>(
    artifacts: &RunArtifacts,
    report: &RunReport<'_>,
    summary: &S,
    environment: &E,
    cleanup: &C,
    write_timeline: bool,
) -> Result<(), String> {
    artifacts
        .write_json(files::CLEANUP, cleanup)
        .map_err(|e| format!("cannot write cleanup.json: {e}"))?;
    artifacts
        .write_metadata_json(
            files::ENVIRONMENT,
            environment,
            report.stack,
            report.app_artifact,
        )
        .map_err(|e| format!("cannot write environment.json: {e}"))?;
    artifacts
        .write_metadata_json(files::SUMMARY, summary, report.stack, report.app_artifact)
        .map_err(|e| format!("cannot write summary.json: {e}"))?;
    if write_timeline {
        artifacts
            .write_text(files::TIMELINE, &render_timeline(report))
            .map_err(|e| format!("cannot write timeline.md: {e}"))?;
    }
    info!(run = %report.run_id, command = report.command, passed = report.passed, "finished");
    Ok(())
}

/// Markdown timeline: one row per check plus the verdict.
pub fn render_timeline(report: &RunReport<'_>) -> String {
    let mut out = format!(
        "# {} {} (lane {}",
        report.command, report.run_id, report.lane
    );
    if let Some(profile) = report.app_profile {
        out.push_str(&format!(", profile {profile}"));
    }
    out.push_str(")\n\n| step | status | detail |\n|---|---|---|\n");
    for check in &report.checks.checks {
        out.push_str(&format!(
            "| {} | {} | {} |\n",
            check.name,
            check.status,
            check.detail.replace('|', "\\|")
        ));
    }
    out.push_str(&format!(
        "\nresult: {}\n",
        if report.passed { "PASS" } else { "FAIL" }
    ));
    out
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use mortsom_common::Redactor;
    use mortsom_common::check::CheckStatus;

    #[test]
    fn writes_all_four_artifacts_with_a_rendered_timeline() {
        let dir = tempfile::tempdir().unwrap();
        let run_id: RunId = "20260815T000000Z-abcdef".parse().unwrap();
        let artifacts = RunArtifacts::create(dir.path(), run_id.clone(), Redactor::new()).unwrap();
        let mut checks = DoctorReport::default();
        checks.push("relay-start", CheckStatus::Pass, "ws://127.0.0.1:1|2");
        let report = RunReport {
            run_id: &run_id,
            lane: Lane::Ci,
            command: "relay smoke",
            app_profile: Some("mobile-legacy"),
            checks: &checks,
            passed: true,
            stack: None,
            app_artifact: None,
        };

        write_report(
            &artifacts,
            &report,
            &serde_json::json!({"a": 1}),
            &serde_json::json!({}),
            &serde_json::json!([]),
            true,
        )
        .unwrap();

        let timeline = std::fs::read_to_string(artifacts.file(files::TIMELINE).unwrap()).unwrap();
        assert!(
            timeline.starts_with(
                "# relay smoke 20260815T000000Z-abcdef (lane ci, profile mobile-legacy)"
            )
        );
        assert!(timeline.contains("| relay-start | PASS | ws://127.0.0.1:1\\|2 |"));
        assert!(timeline.ends_with("result: PASS\n"));
        for name in [files::SUMMARY, files::ENVIRONMENT, files::CLEANUP] {
            assert!(artifacts.file(name).unwrap().is_file());
        }
    }
}
