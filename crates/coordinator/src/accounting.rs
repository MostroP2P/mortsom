//! `mortsom accounting` (spec §5, Phase 6.5): the spend and outcome trend
//! across runs, read from each run's `summary.json`. It reports what the
//! runs recorded; it never queries wallets and never moves value.

use std::path::{Path, PathBuf};

use mortsom_common::Lane;
use serde::{Deserialize, Serialize};
use tracing::warn;

use crate::artifacts::files;
use crate::run::RunSummary;

/// JSON report file name.
pub const ACCOUNTING_JSON: &str = "accounting.json";
/// Markdown report file name.
pub const ACCOUNTING_MD: &str = "accounting.md";

/// One run's line in the trend.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunLine {
    /// Run id (sorted ascending, ids start with a UTC timestamp).
    pub run_id: String,
    /// Lane.
    pub lane: Lane,
    /// App profile.
    pub app_profile: String,
    /// Whether the run passed.
    pub passed: bool,
    /// Scenarios passed.
    pub scenarios_passed: usize,
    /// Scenarios failed.
    pub scenarios_failed: usize,
    /// Scenarios skipped.
    pub scenarios_skipped: usize,
    /// Sats settled by the run's wallets.
    pub settled_sats: u64,
    /// Fees settled.
    pub fee_sats: u64,
    /// Sats still reserved when the run ended (in-flight or unresolved).
    pub reserved_sats: u64,
    /// Orders the cleanup sweep left open.
    pub unresolved_orders: usize,
}

/// The trend report.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccountingReport {
    /// Lane filter applied, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lane: Option<Lane>,
    /// Runs, oldest first.
    pub runs: Vec<RunLine>,
    /// Run directories whose summary was missing or unreadable.
    pub unreadable: Vec<String>,
    /// Sum of settled sats over `runs`.
    pub total_settled_sats: u64,
    /// Sum of fees over `runs`.
    pub total_fee_sats: u64,
    /// Runs that passed.
    pub runs_passed: usize,
    /// Runs still holding reservations or open orders at the end.
    pub runs_with_leftovers: usize,
}

impl AccountingReport {
    /// Markdown rendering.
    pub fn render(&self) -> String {
        let mut out = String::from("# Mortsom accounting\n\n");
        if let Some(lane) = self.lane {
            out.push_str(&format!("Lane: `{lane}`\n\n"));
        }
        out.push_str(&format!(
            "Runs: {} ({} passed, {} with leftovers) — settled {} sats, fees {} sats\n\n",
            self.runs.len(),
            self.runs_passed,
            self.runs_with_leftovers,
            self.total_settled_sats,
            self.total_fee_sats
        ));
        out.push_str("| run | lane | profile | result | scenarios (pass/fail/skip) | settled | fees | reserved | open orders |\n");
        out.push_str("|---|---|---|---|---|---|---|---|---|\n");
        for r in &self.runs {
            out.push_str(&format!(
                "| {} | {} | {} | {} | {}/{}/{} | {} | {} | {} | {} |\n",
                r.run_id,
                r.lane,
                r.app_profile,
                if r.passed { "pass" } else { "FAIL" },
                r.scenarios_passed,
                r.scenarios_failed,
                r.scenarios_skipped,
                r.settled_sats,
                r.fee_sats,
                r.reserved_sats,
                r.unresolved_orders
            ));
        }
        if !self.unreadable.is_empty() {
            out.push_str("\nUnreadable run directories (no usable summary.json):\n");
            for dir in &self.unreadable {
                out.push_str(&format!("- {dir}\n"));
            }
        }
        out
    }
}

/// Builds the report from every `<artifacts_dir>/*/summary.json` produced by
/// `mortsom run`, optionally restricted to `lane`, and writes it next to
/// them as JSON and Markdown.
pub fn build_accounting(
    artifacts_dir: &Path,
    lane: Option<Lane>,
) -> Result<AccountingReport, String> {
    let mut runs = Vec::new();
    let mut unreadable = Vec::new();
    let entries = std::fs::read_dir(artifacts_dir)
        .map_err(|e| format!("cannot read {}: {e}", artifacts_dir.display()))?;
    let mut dirs: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect();
    dirs.sort();
    for dir in dirs {
        let name = dir
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        match read_run(&dir) {
            Some(Some(line)) => {
                if lane.is_none_or(|l| l == line.lane) {
                    runs.push(line);
                }
            }
            // Other commands' summaries (doctor, smoke) are not runs.
            Some(None) => {}
            None => unreadable.push(name),
        }
    }
    let report = AccountingReport {
        lane,
        total_settled_sats: runs.iter().map(|r| r.settled_sats).sum(),
        total_fee_sats: runs.iter().map(|r| r.fee_sats).sum(),
        runs_passed: runs.iter().filter(|r| r.passed).count(),
        runs_with_leftovers: runs
            .iter()
            .filter(|r| r.reserved_sats > 0 || r.unresolved_orders > 0)
            .count(),
        runs,
        unreadable,
    };
    let json = serde_json::to_string_pretty(&report).map_err(|e| e.to_string())?;
    std::fs::write(artifacts_dir.join(ACCOUNTING_JSON), json)
        .map_err(|e| format!("cannot write {ACCOUNTING_JSON}: {e}"))?;
    std::fs::write(artifacts_dir.join(ACCOUNTING_MD), report.render())
        .map_err(|e| format!("cannot write {ACCOUNTING_MD}: {e}"))?;
    Ok(report)
}

/// `None`: no readable summary; `Some(None)`: a summary of another command.
fn read_run(dir: &Path) -> Option<Option<RunLine>> {
    let path = dir.join(files::SUMMARY);
    let text = std::fs::read_to_string(&path).ok()?;
    let value: serde_json::Value = serde_json::from_str(&text)
        .map_err(|e| warn!(path = %path.display(), error = %e, "unreadable summary"))
        .ok()?;
    if value.get("command").and_then(|c| c.as_str()) != Some("run") {
        return Some(None);
    }
    let summary: RunSummary = serde_json::from_value(value)
        .map_err(|e| warn!(path = %path.display(), error = %e, "unreadable run summary"))
        .ok()?;
    let budget = summary.budget.as_ref();
    Some(Some(RunLine {
        run_id: summary.run_id.as_str().to_owned(),
        lane: summary.lane,
        app_profile: summary.app_profile.clone(),
        passed: summary.passed,
        scenarios_passed: summary
            .results
            .iter()
            .filter(|r| r.outcome.is_passed())
            .count(),
        scenarios_failed: summary
            .results
            .iter()
            .filter(|r| {
                matches!(
                    r.outcome,
                    mortsom_scenario_kit::outcome::Outcome::Failed { .. }
                )
            })
            .count(),
        scenarios_skipped: summary
            .results
            .iter()
            .filter(|r| {
                matches!(
                    r.outcome,
                    mortsom_scenario_kit::outcome::Outcome::Skipped { .. }
                )
            })
            .count(),
        settled_sats: budget.map_or(0, |b| b.settled_sats),
        fee_sats: budget.map_or(0, |b| b.fee_sats),
        reserved_sats: budget.map_or(0, |b| b.reserved_sats),
        unresolved_orders: summary.cleanup.as_ref().map_or(0, |c| c.unresolved.len()),
    }))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    fn summary(run_id: &str, lane: &str, passed: bool, settled: u64, unresolved: &str) -> String {
        format!(
            r#"{{
  "run_id": "{run_id}", "lane": "{lane}", "command": "run", "app_profile": "mobile-legacy",
  "app_version": "1.3.2", "mainnet_cleared": true, "relay": null, "actors": [],
  "results": [
    {{"name": "happy_sell", "app_profile": "mobile-legacy", "app_version": null,
     "outcome": {{{}}}, "attempts": 1, "duration": {{"secs": 60, "nanos": 0}}}},
    {{"name": "x", "app_profile": "mobile-legacy", "app_version": null,
     "outcome": {{"outcome": "skipped", "reason": "cap"}}, "attempts": 0, "duration": {{"secs": 0, "nanos": 0}}}}
  ],
  "checks": {{"checks": []}},
  "cleanup": {{"entries": [], "unresolved": [{unresolved}]}},
  "budget": {{"reserved_sats": 0, "settled_sats": {settled}, "fee_sats": 1, "remaining_sats": 100}},
  "farm_teardown": [], "relay_teardown": null, "passed": {passed}
}}"#,
            if passed {
                r#""outcome": "passed""#
            } else {
                r#""outcome": "skipped", "reason": "cap""#
            }
        )
    }

    #[test]
    fn aggregates_run_summaries_and_reports_unreadable_ones() {
        let dir = tempfile::tempdir().unwrap();
        for (id, lane, passed, settled, unresolved) in [
            ("20260815T100000Z-aaaaaa", "canary", true, 1000, ""),
            ("20260815T110000Z-bbbbbb", "canary", false, 0, "\"o1\""),
            ("20260815T120000Z-cccccc", "ci", true, 0, ""),
        ] {
            let run = dir.path().join(id);
            std::fs::create_dir_all(&run).unwrap();
            std::fs::write(
                run.join(files::SUMMARY),
                summary(id, lane, passed, settled, unresolved),
            )
            .unwrap();
        }
        let doctor = dir.path().join("20260815T130000Z-dddddd");
        std::fs::create_dir_all(&doctor).unwrap();
        std::fs::write(
            doctor.join(files::SUMMARY),
            r#"{"command": "doctor", "run_id": "x"}"#,
        )
        .unwrap();
        let broken = dir.path().join("20260815T140000Z-eeeeee");
        std::fs::create_dir_all(&broken).unwrap();
        std::fs::write(broken.join(files::SUMMARY), "{not json").unwrap();
        std::fs::create_dir_all(dir.path().join("20260815T150000Z-ffffff")).unwrap();

        let all = build_accounting(dir.path(), None).unwrap();
        assert_eq!(all.runs.len(), 3);
        assert_eq!(all.total_settled_sats, 1000);
        assert_eq!(all.total_fee_sats, 3);
        assert_eq!(all.runs_passed, 2);
        assert_eq!(all.runs_with_leftovers, 1);
        assert_eq!(all.unreadable.len(), 2, "broken json and empty dir");
        assert_eq!(all.runs[1].scenarios_skipped, 2);
        assert_eq!(all.runs[1].unresolved_orders, 1);

        let canary = build_accounting(dir.path(), Some(Lane::Canary)).unwrap();
        assert_eq!(canary.runs.len(), 2);
        assert!(canary.runs.iter().all(|r| r.lane == Lane::Canary));
        let md = std::fs::read_to_string(dir.path().join(ACCOUNTING_MD)).unwrap();
        assert!(md.contains(
            "| 20260815T110000Z-bbbbbb | canary | mobile-legacy | FAIL | 0/0/2 | 0 | 1 | 0 | 1 |"
        ));
        let json: AccountingReport = serde_json::from_str(
            &std::fs::read_to_string(dir.path().join(ACCOUNTING_JSON)).unwrap(),
        )
        .unwrap();
        assert_eq!(json, canary);
    }

    #[test]
    fn missing_artifacts_dir_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        assert!(build_accounting(&dir.path().join("nope"), None).is_err());
    }
}
