//! `mortsom doctor`: prerequisites, secrets, daemon, relay port and wallets.
//! It performs no payment and creates no order (spec Phase 0.3).

mod host;
mod net;
mod report;
mod tools;

use std::path::Path;

pub use host::{CommandOutput, HostProbe, RealHost};
pub use net::{NetProbes, PortState, RealNet};
pub use report::{CheckResult, CheckStatus, DoctorReport};

use crate::config::{Config, ConfigDiagnosis};

/// Doctor options.
#[derive(Debug, Clone, Default)]
pub struct DoctorOptions {
    /// Skip every check that would open a network connection.
    pub no_network: bool,
}

/// Runs the doctor against `config_path` using the given probes.
pub async fn run_doctor(
    config_path: &Path,
    options: &DoctorOptions,
    host: &dyn HostProbe,
    net: &dyn NetProbes,
) -> DoctorReport {
    let mut report = DoctorReport::default();
    let diagnosis = Config::diagnose(config_path, |name| host.env(name));
    report_config(config_path, &diagnosis, &mut report);
    let config = diagnosis.config.as_ref();

    tools::check_tools(
        host,
        config.map(|c| &c.android),
        config.map(|c| &c.mobile),
        &mut report,
    )
    .await;

    if options.no_network {
        report.push("network", CheckStatus::Skip, "--no-network");
        return report;
    }
    let Some(config) = config else {
        report.push("network", CheckStatus::Skip, "configuration did not load");
        return report;
    };
    net::check_relay_port(net, config.relay.host_port, &mut report).await;
    match config.mobile.mostro_pubkey() {
        Some(pubkey) => net::check_daemon(net, &config.mostro.relays, pubkey, &mut report).await,
        None => report.push(
            "mostro-daemon",
            CheckStatus::Skip,
            "mostro pubkey not resolved",
        ),
    }
    net::check_wallet(net, "alice", &config.wallets.alice_nwc, &mut report).await;
    net::check_wallet(net, "bob", &config.wallets.bob_nwc, &mut report).await;
    report
}

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
