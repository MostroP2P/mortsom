//! The `[relay]`, `[mostro]` and `[wallets]` configuration sections.

use std::str::FromStr;

use mortsom_common::Secret;
use nostr_sdk::prelude::{NostrWalletConnectUri, RelayUrl};
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
        if self.container_name_prefix.is_empty()
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
    /// Relay URLs the daemon is expected to use, as seen from the host.
    pub relays: Vec<String>,
}

fn default_mostro_mode() -> MostroMode {
    MostroMode::ExternalLocal
}
fn default_health_timeout() -> u64 {
    60
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

/// Wallet integration used by both actors.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WalletMode {
    /// Nostr Wallet Connect configured through the app's settings UI.
    Nwc,
}

/// `[wallets]` section: dedicated, funded test wallets. Never personal ones.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WalletsConfig {
    /// Wallet integration.
    #[serde(default = "default_wallet_mode")]
    pub mode: WalletMode,
    /// Alice's NWC URI (secret; `env:` reference in TOML).
    pub alice_nwc: Secret,
    /// Bob's NWC URI (secret; `env:` reference in TOML).
    pub bob_nwc: Secret,
}

fn default_wallet_mode() -> WalletMode {
    WalletMode::Nwc
}

/// Configuration keys whose raw TOML value must be an `env:` reference: the
/// wallet secrets and the daemon settings path (spec §16.4 reads it "from a
/// secret environment reference"). This applies to every layer, including
/// the gitignored local overlay: secrets only ever come from the environment.
pub const ENV_ONLY_KEYS: &[&str] = &[
    "wallets.alice_nwc",
    "wallets.bob_nwc",
    "mostro.settings_file",
];

impl WalletsConfig {
    /// Appends every problem with this section to `errors`. Never includes a
    /// URI in a message.
    pub fn validate(&self, errors: &mut Vec<String>) {
        validate_nwc("wallets.alice_nwc", &self.alice_nwc, errors);
        validate_nwc("wallets.bob_nwc", &self.bob_nwc, errors);
        if !self.alice_nwc.is_empty() && self.alice_nwc == self.bob_nwc {
            errors
                .push("wallets.alice_nwc and wallets.bob_nwc must be different wallets".to_owned());
        }
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
        assert_eq!(config.health_timeout_secs, 60);
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

    #[test]
    fn wallets_accept_two_distinct_valid_uris() {
        let config = WalletsConfig {
            mode: WalletMode::Nwc,
            alice_nwc: Secret::new(ALICE),
            bob_nwc: Secret::new(BOB),
        };

        assert!(collect(|e| config.validate(e)).is_empty());
    }

    #[test]
    fn wallets_reject_identical_uris_without_leaking_them() {
        let config = WalletsConfig {
            mode: WalletMode::Nwc,
            alice_nwc: Secret::new(ALICE),
            bob_nwc: Secret::new(ALICE),
        };

        let errors = collect(|e| config.validate(e));

        assert_eq!(errors.len(), 1);
        assert!(errors[0].contains("must be different"));
        assert!(!errors.iter().any(|e| e.contains("secret=")));
    }

    #[test]
    fn wallets_report_empty_and_invalid_uris() {
        let config = WalletsConfig {
            mode: WalletMode::Nwc,
            alice_nwc: Secret::new(""),
            bob_nwc: Secret::new("https://example.com"),
        };

        let errors = collect(|e| config.validate(e));

        assert_eq!(errors.len(), 2, "{errors:?}");
        assert!(errors[0].contains("unresolved secret"));
        assert!(errors[1].contains("not a valid"));
    }

    #[test]
    fn wallets_debug_output_is_redacted() {
        let config = WalletsConfig {
            mode: WalletMode::Nwc,
            alice_nwc: Secret::new(ALICE),
            bob_nwc: Secret::new(BOB),
        };

        assert!(!format!("{config:?}").contains("secret="));
    }
}
