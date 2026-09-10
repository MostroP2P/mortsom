//! `mortsom cleanup <run>` for a run that left no recovery journal (spec
//! Phase 5.5): report what that run's own sweep could not resolve. Clients
//! and their storage are owned by the recovery journal when one exists; this
//! command never claims more than it verified.

use std::path::Path;

use mortsom_common::RunId;
use mortsom_scenario_kit::cleanup::CleanupReport;
use serde::{Deserialize, Serialize};
use tracing::{info, warn};

use crate::artifacts::files;
use crate::config::MaintenanceConfig as Config;

pub mod lookalike;

/// File the command writes into the run directory when it exists.
pub const CLEANUP_COMMAND_FILE: &str = "cleanup-command.json";

/// Machine-readable outcome.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CleanupRunSummary {
    /// Run being cleaned.
    pub run_id: String,
    /// Whether `<artifacts_dir>/<run_id>` exists.
    pub run_dir_found: bool,
    /// Whether the run wrote `cleanup.json`.
    pub cleanup_report_found: bool,
    /// Orders the run's own sweep left open; only a client session can
    /// close them, so they are reported, not fixed.
    pub unresolved_orders: Vec<String>,
    /// True when the run's sweep ran and left no order unresolved.
    pub clean: bool,
}

impl CleanupRunSummary {
    /// Human-readable rendering.
    pub fn render(&self) -> String {
        let mut out = format!("cleanup of run {}\n", self.run_id);
        if !self.run_dir_found {
            out.push_str("  run directory: not found; orders UNKNOWN\n");
        } else if !self.cleanup_report_found {
            out.push_str(
                "  cleanup.json: missing; the run did not reach its sweep, orders UNKNOWN: check the maker's app\n",
            );
        }
        for order in &self.unresolved_orders {
            out.push_str(&format!(
                "  UNRESOLVED order {order}: cancel it from the maker's app; this command cannot\n"
            ));
        }
        out.push_str("  clients: no recovery journal retained for this run; nothing to reopen\n");
        out.push_str(if self.clean {
            "result: clean\n"
        } else {
            "result: NOT clean (see above)\n"
        });
        out
    }
}

/// Reports on `run_id` (validated: a single safe path segment).
pub fn cleanup_run(config: &Config, run_id: &RunId) -> CleanupRunSummary {
    let run_id = run_id.as_str();
    let run_dir = Path::new(&config.run.artifacts_dir).join(run_id);
    let run_dir_found = run_dir.is_dir();
    let (cleanup_report_found, unresolved_orders) = read_unresolved(&run_dir);
    // Orders count as verified only when the run's own sweep reported them;
    // a run that died before its sweep leaves them unknown, not clean.
    let clean = cleanup_report_found && unresolved_orders.is_empty();
    let summary = CleanupRunSummary {
        run_id: run_id.to_owned(),
        run_dir_found,
        cleanup_report_found,
        unresolved_orders,
        clean,
    };
    if run_dir_found {
        write_summary(&run_dir, &summary);
    }
    info!(
        run = run_id,
        clean = summary.clean,
        "cleanup command finished"
    );
    summary
}

/// Unresolved order ids from the run's `cleanup.json`, if it exists.
fn read_unresolved(run_dir: &Path) -> (bool, Vec<String>) {
    let path = run_dir.join(files::CLEANUP);
    let Ok(text) = std::fs::read_to_string(&path) else {
        return (false, Vec::new());
    };
    match serde_json::from_str::<CleanupReport>(&text) {
        Ok(report) => (true, report.unresolved),
        Err(e) => {
            warn!(path = %path.display(), error = %e, "cleanup.json is unreadable");
            (true, vec![format!("<unreadable {}: {e}>", files::CLEANUP)])
        }
    }
}

fn write_summary(run_dir: &Path, summary: &CleanupRunSummary) {
    let path = run_dir.join(CLEANUP_COMMAND_FILE);
    match serde_json::to_string_pretty(summary) {
        Ok(json) => {
            if let Err(e) = std::fs::write(&path, json) {
                warn!(path = %path.display(), error = %e, "cannot write cleanup summary");
            }
        }
        Err(e) => warn!(error = %e, "cannot serialize cleanup summary"),
    }
}

#[cfg(test)]
mod tests;
