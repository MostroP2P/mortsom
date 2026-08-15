//! Daemon settings checks for the doctor: file presence, parse, identity,
//! relay list, transport and declared timeouts (spec §16.4).

use std::path::Path;

use mortsom_common::check::{CheckStatus, DoctorReport};
use mortsom_environment::daemon_settings::{DaemonSettings, TimeoutLimits, evaluate_settings};
use nostr_sdk::prelude::PublicKey;

use super::host::HostProbe;

/// Loads the daemon settings and evaluates them.
pub fn check_daemon_settings(
    host: &dyn HostProbe,
    settings_file: &str,
    expected_pubkey: Option<PublicKey>,
    mortsom_relays: &[String],
    report: &mut DoctorReport,
) {
    if settings_file.is_empty() {
        report.push(
            "daemon-settings",
            CheckStatus::Skip,
            "settings path not resolved",
        );
        return;
    }
    let path = Path::new(settings_file);
    if !host.exists(path) {
        report.push(
            "daemon-settings",
            CheckStatus::Fail,
            format!("{settings_file} does not exist"),
        );
        return;
    }
    let settings = match DaemonSettings::load(path) {
        Ok(settings) => settings,
        Err(e) => {
            report.push("daemon-settings", CheckStatus::Fail, e.to_string());
            return;
        }
    };
    report.push(
        "daemon-settings",
        CheckStatus::Pass,
        settings_file.to_owned(),
    );
    let Some(expected) = expected_pubkey else {
        report.push(
            "daemon-pubkey",
            CheckStatus::Skip,
            "mostro pubkey not resolved",
        );
        return;
    };
    evaluate_settings(
        &settings,
        expected,
        mortsom_relays,
        TimeoutLimits::default(),
        |name| host.env(name),
        report,
    );
}
