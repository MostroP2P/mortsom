//! The `[relay]`, `[mostro]` and `[wallets]` configuration sections.

use std::str::FromStr;

use mortsom_common::Secret;
use nostr_sdk::prelude::{NostrWalletConnectUri, RelayUrl};

use crate::lnd_target;
use serde::{Deserialize, Serialize};

/// `[relay]` section: the run-owned `nostr-rs-relay` container.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RelayConfig {
    /// Docker image for the relay.
    #[serde(default = "default_relay_image")]
    pub docker_image: String,
    /// Prefix for the per-run container name.
    #[serde(default = "default_container_prefix")]
    pub container_name_prefix: String,
    /// First host port tried; each run allocates a free port at or above it.
    #[serde(default = "default_host_port")]
    pub host_port: u16,
    /// Port the relay listens on inside the container.
    #[serde(default = "default_container_port")]
    pub container_port: u16,
}

fn default_relay_image() -> String {
    "scsibug/nostr-rs-relay:latest".to_owned()
}
fn default_container_prefix() -> String {
    "mortsom-relay".to_owned()
}
fn default_host_port() -> u16 {
    7000
}
fn default_container_port() -> u16 {
    8080
}

impl Default for RelayConfig {
    fn default() -> Self {
        Self {
            docker_image: default_relay_image(),
            container_name_prefix: default_container_prefix(),
            host_port: default_host_port(),
            container_port: default_container_port(),
        }
    }
}

impl RelayConfig {
    /// Appends every problem with this section to `errors`.
    pub fn validate(&self, errors: &mut Vec<String>) {
        if self.docker_image.is_empty() {
            errors.push("relay.docker_image must not be empty".to_owned());
        }
        // Docker requires [a-zA-Z0-9][a-zA-Z0-9_.-]*: a name starting with a
        // separator is refused by the daemon, not by us, and much later.
        let starts_well = self
            .container_name_prefix
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_alphanumeric());
        if !starts_well
            || !self
                .container_name_prefix
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
        {
            errors.push(
                "relay.container_name_prefix must be a non-empty Docker-safe name".to_owned(),
            );
        }
        if self.host_port == 0 {
            errors.push("relay.host_port must be greater than zero".to_owned());
        }
        if self.container_port == 0 {
            errors.push("relay.container_port must be greater than zero".to_owned());
        }
    }
}

/// How the Mostro daemon under test is operated.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MostroMode {
    /// A locally running daemon owned by the operator; Mortsom checks it and
    /// never starts or restarts it.
    ExternalLocal,
}

/// `[mostro]` section: the locally operated daemon.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MostroConfig {
    /// Daemon operation mode.
    #[serde(default = "default_mostro_mode")]
    pub mode: MostroMode,
    /// Path to the daemon's `settings.toml`; read only for pre-flight checks.
    pub settings_file: String,
    /// Maximum time to wait for the daemon's readiness evidence.
    #[serde(default = "default_health_timeout")]
    pub health_timeout_secs: u64,
    /// Relay URLs the daemon is configured with, in the daemon's own view
    /// (e.g. `ws://host.docker.internal:7000` for a containerized daemon).
    /// Mortsom translates host aliases when it dials or compares them.
    pub relays: Vec<String>,
}

fn default_mostro_mode() -> MostroMode {
    MostroMode::ExternalLocal
}
fn default_health_timeout() -> u64 {
    120
}

impl MostroConfig {
    /// Appends every problem with this section to `errors`.
    pub fn validate(&self, errors: &mut Vec<String>) {
        if self.settings_file.is_empty() {
            errors.push("mostro.settings_file must not be empty".to_owned());
        }
        if self.health_timeout_secs == 0 {
            errors.push("mostro.health_timeout_secs must be greater than zero".to_owned());
        }
        if self.relays.is_empty() {
            errors.push("mostro.relays must list at least one relay".to_owned());
        }
        for relay in &self.relays {
            if RelayUrl::parse(relay).is_err() {
                errors.push(format!(
                    "mostro.relays entry `{relay}` is not a valid relay URL"
                ));
            }
        }
    }
}

/// Wallet backend for one actor, discriminated by `kind`.
///
/// The two backends are alternatives, never a combination: an actor has
/// exactly one wallet. `deny_unknown_fields` is what makes that a rule
/// rather than a convention — without it, leaving a `uri` behind while
/// moving an actor to `kind = "lnd"` parses happily and ignores it, so the
/// config says one thing and the run does another.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase", deny_unknown_fields)]
pub enum WalletConfig {
    /// Nostr Wallet Connect, configured into the app through its settings
    /// UI. The app itself pays and issues invoices; Mortsom only observes.
    Nwc {
        /// The actor's NWC URI (secret; `env:` reference in TOML).
        uri: Secret,
    },
    /// Direct access to an LND node over gRPC, the way any Lightning client
    /// connects: endpoint, TLS certificate and macaroon.
    ///
    /// This is how a local [Polar](https://lightningpolar.com/) network is
    /// driven. The app has no wallet connected on this path, so Mortsom
    /// pays the hold invoice and issues the buyer invoice itself, through
    /// the app's manual controls — which is what makes a whole trade settle
    /// in a fraction of a second, on regtest, with worthless sats.
    Lnd {
        /// gRPC endpoint. A missing scheme is read as `https://`, so
        /// Polar's own `127.0.0.1:10001` can be pasted as shown.
        host: String,
        /// Path to the macaroon granting Mortsom its permissions.
        /// A leading `~` is expanded.
        macaroon: Secret,
        /// Path to the node's TLS certificate. A leading `~` is expanded.
        cert: Secret,
    },
}

impl WalletConfig {
    /// Short name of the backend, for reports and evidence.
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Nwc { .. } => "nwc",
            Self::Lnd { .. } => "lnd",
        }
    }

    /// Whether Mortsom drives the app's manual invoice and payment controls
    /// rather than its connected wallet.
    pub fn drives_manual_payments(&self) -> bool {
        matches!(self, Self::Lnd { .. })
    }

    /// Appends every problem with this wallet to `errors`. Never includes a
    /// URI, a macaroon or a certificate in a message.
    pub fn validate(&self, key: &str, errors: &mut Vec<String>) {
        match self {
            Self::Nwc { uri } => validate_nwc(&format!("{key}.uri"), uri, errors),
            Self::Lnd {
                host,
                macaroon,
                cert,
            } => {
                if host.trim().is_empty() {
                    errors.push(format!("{key}.host is empty (unresolved secret)"));
                } else if !lnd_target::host_has_a_port(host) {
                    // LND's gRPC port is never the default, so a host
                    // without one connects somewhere else rather than
                    // failing honestly.
                    errors.push(format!(
                        "{key}.host ({host:?}) has no port; LND's gRPC port is never the default"
                    ));
                }
                for (field, value) in [("macaroon", macaroon), ("cert", cert)] {
                    validate_credential_file(&format!("{key}.{field}"), value, errors);
                }
            }
        }
    }
}

/// A credential path must exist at load time.
///
/// The network number in a Polar path (`networks/1`) changes with every
/// network created, so a stale path is *the* failure mode — and it would
/// otherwise surface as an opaque gRPC error mid-run. The message names the
/// path with the `~` already expanded, which is the path that was looked
/// for.
fn validate_credential_file(key: &str, value: &Secret, errors: &mut Vec<String>) {
    if value.is_empty() {
        errors.push(format!("{key} is empty (unresolved secret)"));
        return;
    }
    let path = lnd_target::credential_path(value.expose());
    if !path.exists() {
        errors.push(format!("{key} file not found: {}", path.display()));
    }
}

/// `[wallets]` section: dedicated, funded test wallets. Never personal ones.
///
/// Each actor names its own backend, so a run can mix them — useful when
/// only one side of a trade needs the controls direct node access gives.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WalletsConfig {
    /// Alice's wallet.
    pub alice: WalletConfig,
    /// Bob's wallet.
    pub bob: WalletConfig,
}

/// Configuration keys whose raw TOML value must be an `env:` reference: the
/// wallet secrets and the daemon settings path (spec §16.4 reads it "from a
/// secret environment reference"). This applies to every layer, including
/// the gitignored local overlay: secrets only ever come from the environment.
pub const ENV_ONLY_KEYS: &[&str] = &[
    "wallets.alice.uri",
    "wallets.alice.macaroon",
    "wallets.alice.cert",
    "wallets.bob.uri",
    "wallets.bob.macaroon",
    "wallets.bob.cert",
    "mostro.settings_file",
];

impl WalletsConfig {
    /// Appends every problem with this section to `errors`. Never includes a
    /// URI, a macaroon or a certificate in a message.
    pub fn validate(&self, errors: &mut Vec<String>) {
        self.alice.validate("wallets.alice", errors);
        self.bob.validate("wallets.bob", errors);
        if self.alice == self.bob && !is_unresolved(&self.alice) {
            // One wallet for both actors would have each side of a trade
            // paying itself, and every value assertion would be meaningless.
            errors.push("wallets.alice and wallets.bob must be different wallets".to_owned());
        }
    }

    /// The wallet for `label` (`alice` or `bob`).
    pub fn for_label(&self, label: &str) -> Option<&WalletConfig> {
        match label {
            "alice" => Some(&self.alice),
            "bob" => Some(&self.bob),
            _ => None,
        }
    }

    /// Every credential in this section, labelled, so redactors and
    /// evidence sinks can be told about all of them in one place. A backend
    /// added without being listed here would leak into artifacts.
    pub fn secrets(&self) -> Vec<&Secret> {
        let mut out = Vec::new();
        for wallet in [&self.alice, &self.bob] {
            match wallet {
                WalletConfig::Nwc { uri } => out.push(uri),
                WalletConfig::Lnd { macaroon, cert, .. } => {
                    out.push(macaroon);
                    out.push(cert);
                }
            }
        }
        out
    }
}

/// An unresolved wallet is empty on both sides and already reported as
/// such; calling it a duplicate on top would be noise.
fn is_unresolved(wallet: &WalletConfig) -> bool {
    match wallet {
        WalletConfig::Nwc { uri } => uri.is_empty(),
        WalletConfig::Lnd { host, .. } => host.trim().is_empty(),
    }
}

fn validate_nwc(key: &str, uri: &Secret, errors: &mut Vec<String>) {
    if uri.is_empty() {
        errors.push(format!("{key} is empty (unresolved secret)"));
    } else if NostrWalletConnectUri::from_str(uri.expose()).is_err() {
        errors.push(format!("{key} is not a valid nostr+walletconnect:// URI"));
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    const ALICE: &str = "nostr+walletconnect://b889ff5b1513b641e2a139f661a661364979c5beee91842f8f0ef42ab558e9d4?relay=wss%3A%2F%2Frelay.damus.io&secret=71a8c14c1407c113601079c4302dab36460f0ccd0ad506f1f2dc73b5100e4f3c";
    const BOB: &str = "nostr+walletconnect://b889ff5b1513b641e2a139f661a661364979c5beee91842f8f0ef42ab558e9d4?relay=wss%3A%2F%2Frelay.damus.io&secret=0000000000000000000000000000000000000000000000000000000000000001";

    fn collect(f: impl FnOnce(&mut Vec<String>)) -> Vec<String> {
        let mut errors = Vec::new();
        f(&mut errors);
        errors
    }

    #[test]
    fn relay_defaults_are_valid() {
        let config: RelayConfig = toml::from_str("").unwrap();

        assert_eq!(config, RelayConfig::default());
        assert_eq!(config.host_port, 7000);
        assert_eq!(config.container_port, 8080);
        assert!(collect(|e| config.validate(e)).is_empty());
    }

    #[test]
    fn relay_rejects_bad_names_and_zero_ports() {
        let config = RelayConfig {
            docker_image: String::new(),
            container_name_prefix: "bad name!".into(),
            host_port: 0,
            container_port: 0,
        };

        assert_eq!(collect(|e| config.validate(e)).len(), 4);
    }

    #[test]
    fn mostro_section_parses_and_validates() {
        let config: MostroConfig = toml::from_str(
            "settings_file = \"/tmp/settings.toml\"\nrelays = [\"ws://host.docker.internal:7000\"]",
        )
        .unwrap();

        assert_eq!(config.mode, MostroMode::ExternalLocal);
        assert_eq!(config.health_timeout_secs, 120);
        assert!(collect(|e| config.validate(e)).is_empty());
    }

    #[test]
    fn mostro_reports_every_problem() {
        let config = MostroConfig {
            mode: MostroMode::ExternalLocal,
            settings_file: String::new(),
            health_timeout_secs: 0,
            relays: vec!["not a url".into()],
        };

        assert_eq!(collect(|e| config.validate(e)).len(), 3);
    }

    #[test]
    fn mostro_requires_at_least_one_relay() {
        let config = MostroConfig {
            mode: MostroMode::ExternalLocal,
            settings_file: "x".into(),
            health_timeout_secs: 1,
            relays: vec![],
        };

        assert!(
            collect(|e| config.validate(e))
                .iter()
                .any(|e| e.contains("at least one"))
        );
    }

    #[test]
    fn unknown_mostro_mode_is_rejected() {
        let result: Result<MostroConfig, _> =
            toml::from_str("mode = \"managed\"\nsettings_file = \"x\"\nrelays = []");

        assert!(result.is_err());
    }

    fn nwc(uri: &str) -> WalletConfig {
        WalletConfig::Nwc {
            uri: Secret::new(uri),
        }
    }

    /// An LND wallet whose credential files exist, so only the field under
    /// test is what fails.
    fn lnd_with(host: &str, dir: &std::path::Path) -> WalletConfig {
        let cert = dir.join("tls.cert");
        let macaroon = dir.join("admin.macaroon");
        std::fs::write(&cert, b"cert").unwrap();
        std::fs::write(&macaroon, b"macaroon").unwrap();
        WalletConfig::Lnd {
            host: host.to_owned(),
            macaroon: Secret::new(macaroon.display().to_string()),
            cert: Secret::new(cert.display().to_string()),
        }
    }

    #[test]
    fn wallets_accept_two_distinct_valid_uris() {
        let config = WalletsConfig {
            alice: nwc(ALICE),
            bob: nwc(BOB),
        };

        assert!(collect(|e| config.validate(e)).is_empty());
    }

    #[test]
    fn wallets_reject_identical_uris_without_leaking_them() {
        let config = WalletsConfig {
            alice: nwc(ALICE),
            bob: nwc(ALICE),
        };

        let errors = collect(|e| config.validate(e));

        assert_eq!(errors.len(), 1);
        assert!(errors[0].contains("must be different"));
        assert!(!errors.iter().any(|e| e.contains("secret=")));
    }

    #[test]
    fn wallets_report_empty_and_invalid_uris() {
        let config = WalletsConfig {
            alice: nwc(""),
            bob: nwc("https://example.com"),
        };

        let errors = collect(|e| config.validate(e));

        assert_eq!(errors.len(), 2, "{errors:?}");
        assert!(errors[0].contains("unresolved secret"));
        assert!(errors[1].contains("not a valid"));
    }

    #[test]
    fn wallets_debug_output_is_redacted() {
        let config = WalletsConfig {
            alice: nwc(ALICE),
            bob: nwc(BOB),
        };

        assert!(!format!("{config:?}").contains("secret="));
    }

    /// The two backends are alternatives. Leaving a `uri` behind while
    /// moving an actor to a node must fail, or the config claims one
    /// backend while the run uses another.
    #[test]
    fn a_wallet_cannot_name_both_backends() {
        let result: Result<WalletConfig, _> = toml::from_str(
            "kind = \"lnd\"\nhost = \"127.0.0.1:10001\"\nmacaroon = \"m\"\ncert = \"c\"\nuri = \"nostr+walletconnect://x\"",
        );

        let message = result.unwrap_err().to_string();
        assert!(message.contains("unknown field `uri`"), "{message}");
    }

    #[test]
    fn a_polar_style_lnd_wallet_is_accepted_as_displayed() {
        let dir = tempfile::tempdir().unwrap();
        let config = WalletsConfig {
            alice: nwc(ALICE),
            // Exactly what Polar's Connect tab shows: no scheme.
            bob: lnd_with("127.0.0.1:10001", dir.path()),
        };

        assert!(collect(|e| config.validate(e)).is_empty());
        assert!(config.bob.drives_manual_payments());
        assert!(!config.alice.drives_manual_payments());
        assert_eq!(config.bob.kind(), "lnd");
    }

    /// LND's gRPC port is never the default, so a host without one connects
    /// somewhere else instead of failing honestly.
    #[test]
    fn an_lnd_host_without_a_port_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let config = WalletsConfig {
            alice: nwc(ALICE),
            bob: lnd_with("127.0.0.1", dir.path()),
        };

        let errors = collect(|e| config.validate(e));

        assert_eq!(errors.len(), 1, "{errors:?}");
        assert!(errors[0].contains("no port"), "{errors:?}");
    }

    /// A Polar network number changes with every network created, so a
    /// stale credential path is the usual failure — and it must be named at
    /// load time, not as an opaque gRPC error mid-run.
    #[test]
    fn a_missing_credential_file_is_named_at_load_time() {
        let config = WalletsConfig {
            alice: nwc(ALICE),
            bob: WalletConfig::Lnd {
                host: "127.0.0.1:10001".to_owned(),
                macaroon: Secret::new("/nonexistent/polar/admin.macaroon"),
                cert: Secret::new("/nonexistent/polar/tls.cert"),
            },
        };

        let errors = collect(|e| config.validate(e));

        assert_eq!(errors.len(), 2, "{errors:?}");
        assert!(errors.iter().any(|e| e.contains("wallets.bob.macaroon")
            && e.contains("/nonexistent/polar/admin.macaroon")));
        assert!(
            errors.iter().any(
                |e| e.contains("wallets.bob.cert") && e.contains("/nonexistent/polar/tls.cert")
            )
        );
    }

    /// An NWC actor pays through the app's own wallet, so nothing is driven
    /// manually and the app's NWC controls stay the ones under test.
    #[test]
    fn two_nwc_wallets_drive_no_manual_payments() {
        let config = WalletsConfig {
            alice: nwc(ALICE),
            bob: nwc(BOB),
        };

        assert!(!config.alice.drives_manual_payments());
        assert!(!config.bob.drives_manual_payments());
    }
    /// Docker refuses a container name that does not start with a letter or
    /// a digit, and it refuses it at `docker run`, long after the operator
    /// could have been told.
    #[test]
    fn a_prefix_that_docker_would_refuse_is_rejected() {
        for prefix in ["-relay", ".relay", "_relay", ""] {
            let config = RelayConfig {
                container_name_prefix: prefix.into(),
                ..RelayConfig::default()
            };
            assert!(
                collect(|e| config.validate(e))
                    .iter()
                    .any(|e| e.contains("container_name_prefix")),
                "`{prefix}` should be rejected"
            );
        }
        let config = RelayConfig::default();
        assert!(collect(|e| config.validate(e)).is_empty());
    }
}
