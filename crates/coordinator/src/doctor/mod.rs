//! `mortsom doctor`: prerequisites, secrets, stack, daemon, relay port and
//! wallets. It performs no payment and creates no order (spec Phase 0.3).

mod cli;
pub(crate) mod desktop;
mod desktop_build;
mod host;
mod net;

use std::path::Path;

pub(crate) use desktop::require_build_prerequisites;
pub use desktop::{DesktopDoctorOptions, DesktopPreflight, desktop_preflight, run_desktop_doctor};
pub use host::{CommandOutput, HostProbe, RealHost};
pub use mortsom_common::check::{CheckResult, CheckStatus, DoctorReport};
pub use net::{NetProbes, PortState, RealNet};

use crate::config::ConfigDiagnosis;

fn report_config(path: &Path, diagnosis: &ConfigDiagnosis, report: &mut DoctorReport) {
    match &diagnosis.config {
        Some(config) => report.push(
            "config",
            CheckStatus::Pass,
            format!("{} (lane {})", path.display(), config.run.lane),
        ),
        None => report.push(
            "config",
            CheckStatus::Fail,
            format!("{} did not load", path.display()),
        ),
    }
    for name in &diagnosis.missing_env {
        report.push(format!("env {name}"), CheckStatus::Fail, "not set");
    }
    for problem in &diagnosis.problems {
        report.push("config", CheckStatus::Fail, problem.clone());
    }
}

#[cfg(test)]
mod tests;
