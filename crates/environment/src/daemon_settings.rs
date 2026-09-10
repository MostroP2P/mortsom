//! Read-only inspection of the external Mostro daemon's `settings.toml`
//! (spec §16.4 pre-flight): pubkey identity, relay list and declared
//! timeouts. Only the fields Mortsom needs are modelled; unknown fields are
//! ignored so daemon upgrades do not break the check.

use std::path::{Path, PathBuf};

use mortsom_common::Secret;
use mortsom_common::check::{CheckStatus, DoctorReport};
use nostr_sdk::prelude::{Keys, PublicKey};
use serde::Deserialize;
use thiserror::Error;

use crate::relay_url::any_same_relay;

/// Environment variable the daemon accepts instead of `nsec_privkey`.
pub const DAEMON_NSEC_ENV: &str = "MOSTRO_NSEC_PRIVKEY";

/// Upper bounds for "short enough" test timeouts. Values above these
/// produce a warning, not a failure: they are advisory at pre-flight because
/// only expiration scenarios depend on them, and those scenarios re-check the
/// live values (info event tags) against their own deadlines before running.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TimeoutLimits {
    /// `[mostro].expiration_seconds` — pending order lifetime.
    pub pending_order_secs: u64,
    /// `[lightning].hold_invoice_expiration_window` — time to pay a hold invoice.
    pub hold_invoice_secs: u64,
    /// `[lightning].invoice_expiration_window` — buyer invoice minimum expiry.
    pub buyer_invoice_secs: u64,
    /// `[mostro].expiration_hours` — fiat window for an active order.
    pub fiat_window_hours: u64,
}

impl Default for TimeoutLimits {
    fn default() -> Self {
        Self {
            pending_order_secs: 900,
            hold_invoice_secs: 900,
            buyer_invoice_secs: 3600,
            fiat_window_hours: 24,
        }
    }
}

/// `[nostr]` section of the daemon settings.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct NostrSection {
    /// Private key; may be absent when provided by [`DAEMON_NSEC_ENV`].
    #[serde(default)]
    pub nsec_privkey: Option<Secret>,
    /// Relays the daemon publishes to.
    #[serde(default)]
    pub relays: Vec<String>,
}

/// `[lightning]` section of the daemon settings.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct LightningSection {
    /// Hold invoice payment window in seconds.
    #[serde(default)]
    pub hold_invoice_expiration_window: Option<u64>,
    /// Buyer invoice minimum expiry in seconds.
    #[serde(default)]
    pub invoice_expiration_window: Option<u64>,
    /// gRPC host of the LND node (not a secret, but never printed in full).
    #[serde(default)]
    pub lnd_grpc_host: Option<String>,
}

/// `[mostro]` section of the daemon settings.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct MostroSection {
    /// Pending order lifetime in seconds.
    #[serde(default)]
    pub expiration_seconds: Option<u64>,
    /// Fiat window in hours.
    #[serde(default)]
    pub expiration_hours: Option<u64>,
    /// Transport: `nip44` (v2) or `gift-wrap` (obsolete v1).
    #[serde(default)]
    pub transport: Option<String>,
    /// Relay-list republish interval in seconds.
    #[serde(default)]
    pub publish_relays_interval: Option<u64>,
    /// Info-event republish interval in seconds.
    #[serde(default)]
    pub publish_mostro_info_interval: Option<u64>,
}

/// The subset of the daemon settings Mortsom inspects.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct DaemonSettings {
    /// `[nostr]`.
    #[serde(default)]
    pub nostr: NostrSection,
    /// `[lightning]`.
    #[serde(default)]
    pub lightning: LightningSection,
    /// `[mostro]`.
    #[serde(default)]
    pub mostro: MostroSection,
}

/// Settings loading failures. Messages never include file contents.
#[derive(Debug, Error)]
pub enum DaemonSettingsError {
    /// The file could not be read.
    #[error("cannot read daemon settings {path}: {source}")]
    Read {
        /// Path that failed.
        path: PathBuf,
        /// I/O error.
        #[source]
        source: std::io::Error,
    },
    /// The file is not valid TOML for the expected schema.
    #[error("daemon settings {path} could not be parsed")]
    Parse {
        /// Path that failed.
        path: PathBuf,
    },
}

impl DaemonSettings {
    /// Loads the daemon settings from `path`.
    pub fn load(path: &Path) -> Result<Self, DaemonSettingsError> {
        let text = std::fs::read_to_string(path).map_err(|source| DaemonSettingsError::Read {
            path: path.to_path_buf(),
            source,
        })?;
        Self::from_toml_str(&text).ok_or_else(|| DaemonSettingsError::Parse {
            path: path.to_path_buf(),
        })
    }

    /// Parses settings text; the parse error is discarded so it can never
    /// echo a private key.
    pub fn from_toml_str(text: &str) -> Option<Self> {
        toml::from_str(text).ok()
    }

    /// Derives the daemon public key with the daemon's own precedence: the
    /// [`DAEMON_NSEC_ENV`] variable overrides `nsec_privkey` in the file.
    /// `None` when neither is available or parseable. The key material is
    /// dropped as soon as the public key is derived.
    pub fn derive_pubkey<F>(&self, env: F) -> Option<PublicKey>
    where
        F: Fn(&str) -> Option<String>,
    {
        let nsec = env(DAEMON_NSEC_ENV)
            .filter(|v| !v.is_empty())
            .map(Secret::new)
            .or_else(|| {
                self.nostr
                    .nsec_privkey
                    .clone()
                    .filter(|s| !s.is_empty() && !s.expose().ends_with("..."))
            })?;
        Keys::parse(nsec.expose())
            .ok()
            .map(|keys| keys.public_key())
    }

    /// True when the daemon's relay list contains any of `urls`, treating
    /// host aliases (`localhost`, `127.0.0.1`, `host.docker.internal`) as
    /// the same host.
    pub fn advertises_any_relay(&self, urls: &[String]) -> bool {
        any_same_relay(&self.nostr.relays, urls)
    }
}

/// Evaluates the settings against the expected identity, relay set and
/// timeout limits, appending checks to `report`.
pub fn evaluate_settings<F>(
    settings: &DaemonSettings,
    expected_pubkey: PublicKey,
    mortsom_relays: &[String],
    limits: TimeoutLimits,
    env: F,
    report: &mut DoctorReport,
) where
    F: Fn(&str) -> Option<String>,
{
    match settings.derive_pubkey(env) {
        Some(pubkey) if pubkey == expected_pubkey => {
            report.push("daemon-pubkey", CheckStatus::Pass, "settings identity matches MORTSOM_MOSTRO_PUBKEY")
        }
        Some(_) => report.push(
            "daemon-pubkey",
            CheckStatus::Fail,
            "settings identity differs from MORTSOM_MOSTRO_PUBKEY",
        ),
        None => report.push(
            "daemon-pubkey",
            CheckStatus::Warn,
            "nsec not available in settings or MOSTRO_NSEC_PRIVKEY; identity verified only via the info event author",
        ),
    }
    if settings.advertises_any_relay(mortsom_relays) {
        report.push(
            "daemon-relays",
            CheckStatus::Pass,
            "settings relay list includes the Mortsom relay",
        );
    } else {
        report.push(
            "daemon-relays",
            CheckStatus::Fail,
            format!(
                "settings relay list {:?} does not include any of {:?}",
                settings.nostr.relays, mortsom_relays
            ),
        );
    }
    match settings.mostro.transport.as_deref() {
        None | Some("nip44") => {
            report.push("daemon-transport", CheckStatus::Pass, "nip44 (protocol v2)")
        }
        Some(other) => report.push(
            "daemon-transport",
            CheckStatus::Fail,
            format!("transport `{other}` is not protocol v2"),
        ),
    }
    evaluate_timeouts(settings, limits, report);
}

fn evaluate_timeouts(settings: &DaemonSettings, limits: TimeoutLimits, report: &mut DoctorReport) {
    let checks = [
        (
            "daemon-timeout pending",
            "mostro.expiration_seconds",
            settings.mostro.expiration_seconds,
            limits.pending_order_secs,
            "s",
        ),
        (
            "daemon-timeout hold-invoice",
            "lightning.hold_invoice_expiration_window",
            settings.lightning.hold_invoice_expiration_window,
            limits.hold_invoice_secs,
            "s",
        ),
        (
            "daemon-timeout buyer-invoice",
            "lightning.invoice_expiration_window",
            settings.lightning.invoice_expiration_window,
            limits.buyer_invoice_secs,
            "s",
        ),
        (
            "daemon-timeout fiat-window",
            "mostro.expiration_hours",
            settings.mostro.expiration_hours,
            limits.fiat_window_hours,
            "h",
        ),
    ];
    for (name, key, value, limit, unit) in checks {
        match value {
            None => report.push(
                name,
                CheckStatus::Warn,
                format!("{key} not declared in settings"),
            ),
            Some(v) if v > limit => report.push(
                name,
                CheckStatus::Warn,
                format!("{key} = {v}{unit} exceeds the recommended {limit}{unit} for scenarios"),
            ),
            Some(v) => report.push(name, CheckStatus::Pass, format!("{key} = {v}{unit}")),
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use nostr_sdk::prelude::ToBech32;

    const NSEC: &str = "nsec13as48eum93hkg7plv526r9gjpa0uc52zysqm93pmnkca9e69x6tsdjmdxd";

    fn settings() -> DaemonSettings {
        DaemonSettings::from_toml_str(&format!(
            r#"
            [lightning]
            lnd_grpc_host = 'https://127.0.0.1:10001'
            invoice_expiration_window = 3600
            hold_invoice_expiration_window = 300
            unknown_future_field = 1
            [nostr]
            nsec_privkey = '{NSEC}'
            relays = ['ws://localhost:7000', 'wss://relay.example']
            [mostro]
            expiration_hours = 24
            expiration_seconds = 900
            transport = "nip44"
            "#
        ))
        .unwrap()
    }

    fn expected() -> PublicKey {
        Keys::parse(NSEC).unwrap().public_key()
    }

    #[test]
    fn parses_subset_and_ignores_unknown_fields() {
        let s = settings();
        assert_eq!(s.nostr.relays.len(), 2);
        assert_eq!(s.lightning.hold_invoice_expiration_window, Some(300));
        assert!(format!("{s:?}").contains("<redacted>"));
        assert!(!format!("{s:?}").contains("nsec1"));
    }

    #[test]
    fn env_nsec_overrides_the_settings_file_like_the_daemon_does() {
        let other = Keys::generate();
        let other_nsec = other.secret_key().to_bech32().unwrap();

        let derived =
            settings().derive_pubkey(|name| (name == DAEMON_NSEC_ENV).then(|| other_nsec.clone()));

        assert_eq!(derived, Some(other.public_key()));
    }

    #[test]
    fn derives_pubkey_from_settings_or_env() {
        assert_eq!(settings().derive_pubkey(|_| None), Some(expected()));

        let mut without = settings();
        without.nostr.nsec_privkey = None;
        assert_eq!(without.derive_pubkey(|_| None), None);
        assert_eq!(
            without.derive_pubkey(|name| (name == DAEMON_NSEC_ENV).then(|| NSEC.to_owned())),
            Some(expected())
        );

        let mut placeholder = settings();
        placeholder.nostr.nsec_privkey = Some(Secret::new("nsec1..."));
        assert_eq!(placeholder.derive_pubkey(|_| None), None);
    }

    #[test]
    fn relay_membership_is_url_normalized_across_host_aliases() {
        let s = settings();
        assert!(s.advertises_any_relay(&["ws://localhost:7000/".into()]));
        assert!(s.advertises_any_relay(&["ws://host.docker.internal:7000".into()]));
        assert!(s.advertises_any_relay(&["ws://127.0.0.1:7000".into()]));
        assert!(!s.advertises_any_relay(&["ws://localhost:7001".into()]));
        assert!(!s.advertises_any_relay(&["not a url".into()]));
    }

    #[test]
    fn healthy_settings_pass_every_check() {
        let mut report = DoctorReport::default();

        evaluate_settings(
            &settings(),
            expected(),
            &["ws://localhost:7000".into()],
            TimeoutLimits::default(),
            |_| None,
            &mut report,
        );

        assert!(report.is_healthy(), "{}", report.render());
        assert_eq!(report.count(CheckStatus::Warn), 0);
    }

    #[test]
    fn wrong_identity_missing_relay_and_v1_transport_fail() {
        let mut report = DoctorReport::default();
        let mut s = settings();
        s.mostro.transport = Some("gift-wrap".into());

        evaluate_settings(
            &s,
            Keys::generate().public_key(),
            &["ws://127.0.0.1:9999".into()],
            TimeoutLimits::default(),
            |_| None,
            &mut report,
        );

        assert_eq!(report.count(CheckStatus::Fail), 3, "{}", report.render());
    }

    #[test]
    fn long_or_missing_timeouts_warn() {
        let mut report = DoctorReport::default();
        let mut s = settings();
        s.mostro.expiration_seconds = Some(86_400);
        s.lightning.invoice_expiration_window = None;

        evaluate_settings(
            &s,
            expected(),
            &["ws://localhost:7000".into()],
            TimeoutLimits::default(),
            |_| None,
            &mut report,
        );

        assert!(report.is_healthy());
        assert_eq!(report.count(CheckStatus::Warn), 2, "{}", report.render());
    }

    #[test]
    fn missing_or_invalid_file_errors_do_not_echo_contents() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.toml");
        assert!(matches!(
            DaemonSettings::load(&path),
            Err(DaemonSettingsError::Read { .. })
        ));

        std::fs::write(&path, "[nostr\nnsec_privkey = 'nsec1secret'").unwrap();
        let err = DaemonSettings::load(&path).unwrap_err();
        assert!(matches!(err, DaemonSettingsError::Parse { .. }));
        assert!(!err.to_string().contains("nsec1secret"));
    }
}
