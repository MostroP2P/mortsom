//! `mortsom doctor`: prerequisites, secrets, daemon, relay port and wallets.
//! It performs no payment and creates no order (spec Phase 0.3).

mod host;
mod net;
mod settings;
mod tools;

use std::path::Path;

pub use host::{CommandOutput, HostProbe, RealHost};
pub use mortsom_common::check::{CheckResult, CheckStatus, DoctorReport};
pub use net::{NetProbes, PortState, RealNet};

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
    if let Some(config) = config {
        settings::check_daemon_settings(
            host,
            &config.mostro.settings_file,
            config.mobile.mostro_pubkey(),
            &config.mostro.relays,
            &mut report,
        );
    }

    if options.no_network {
        report.push("network", CheckStatus::Skip, "--no-network");
        return report;
    }
    let Some(config) = config else {
        report.push("network", CheckStatus::Skip, "configuration did not load");
        return report;
    };
    let port_state = net::check_relay_port(net, config.relay.host_port, &mut report).await;
    // Mortsom's own relay, so the daemon checks can tell "no run is live"
    // from "the relay the daemon needs is broken".
    let run_relay = net::RunRelay {
        url: format!("ws://127.0.0.1:{}", config.relay.host_port),
        port_in_use: port_state == net::PortState::InUse,
    };
    match config.mobile.mostro_pubkey() {
        Some(pubkey) => {
            net::check_daemon(
                net,
                &config.mostro.relays,
                pubkey,
                Some(&run_relay),
                &mut report,
            )
            .await;
        }
        None => report.push(
            "mostro-daemon",
            CheckStatus::Skip,
            "mostro pubkey not resolved",
        ),
    }
    let liquidity = net::Liquidity {
        max_order_sats: config.run.max_order_sats,
        max_run_spend_sats: config.run.max_run_spend_sats,
    };
    net::check_wallet(net, "alice", &config.wallets.alice, liquidity, &mut report).await;
    net::check_wallet(net, "bob", &config.wallets.bob, liquidity, &mut report).await;
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
