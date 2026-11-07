//! The `[mobile]` configuration section: which application checkout Mortsom
//! drives and the daemon it must be pointed at.

use std::str::FromStr;

use nostr::key::PublicKey;
use serde::{Deserialize, Serialize};

/// Default application profile (spec §12.2).
pub const DEFAULT_PROFILE: &str = "mobile-v2";
/// The mostro-cli profile (spec §6.3).
pub const CLI_PROFILE: &str = "cli";
/// Profiles Mortsom knows how to drive.
pub const KNOWN_PROFILES: &[&str] = &[DEFAULT_PROFILE, CLI_PROFILE];

/// `[mobile]` section.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MobileConfig {
    /// Named client profile: `mobile-v2` (the app) or `cli` (mostro-cli).
    #[serde(default = "default_profile")]
    pub profile: String,
    /// Explicit local checkout; empty selects managed upstream source.
    #[serde(default)]
    pub source_dir: String,
    /// Managed branch or GitHub PR URL; omitted selects main.
    #[serde(default)]
    pub app_ref: Option<String>,
    /// Public key of the daemon under test (hex or npub). Not a secret.
    pub mostro_pubkey: String,
    /// Seconds after creation every order the test build creates asks the
    /// daemon to expire it at (`MORTSOM_ORDER_EXPIRY_SECS`). Set from the
    /// stack's declared `order_expiration_secs`; `None` leaves the daemon's
    /// own default, which is hour-granular.
    #[serde(default)]
    pub order_expiry_secs: Option<u64>,
}

fn default_profile() -> String {
    DEFAULT_PROFILE.to_owned()
}

impl MobileConfig {
    /// Appends every problem with this section to `errors`.
    pub fn validate(&self, errors: &mut Vec<String>) {
        if !KNOWN_PROFILES.contains(&self.profile.as_str()) {
            errors.push(format!(
                "mobile.profile `{}` is unknown (expected one of {})",
                self.profile,
                KNOWN_PROFILES.join(", ")
            ));
        }
        if !self.source_dir.is_empty() && self.app_ref.is_some() {
            errors.push("mobile.source_dir and mobile.app_ref are mutually exclusive".to_owned());
        }
        if PublicKey::from_str(&self.mostro_pubkey).is_err() {
            errors.push("mobile.mostro_pubkey is not a valid hex or npub public key".to_owned());
        }
    }

    /// The daemon public key, parsed. Callers must have validated first.
    pub fn mostro_pubkey(&self) -> Option<PublicKey> {
        PublicKey::from_str(&self.mostro_pubkey).ok()
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    const HEX: &str = "dbe0b1be7aafd3cfba92d7463edbd4e33b2969f61bd554d37ac56f032e13355a";

    fn valid() -> MobileConfig {
        MobileConfig {
            profile: DEFAULT_PROFILE.into(),
            source_dir: "../mobilev2".into(),
            app_ref: None,
            mostro_pubkey: HEX.into(),
            order_expiry_secs: None,
        }
    }

    fn errors_of(config: &MobileConfig) -> Vec<String> {
        let mut errors = Vec::new();
        config.validate(&mut errors);
        errors
    }

    #[test]
    fn valid_section_has_no_errors() {
        assert!(errors_of(&valid()).is_empty());
        assert!(valid().mostro_pubkey().is_some());
    }

    #[test]
    fn defaults_apply_when_deserializing() {
        let config: MobileConfig = toml::from_str(&format!(
            "mostro_pubkey = \"{HEX}\"\nsource_dir = \"../mobilev2\""
        ))
        .unwrap();

        assert_eq!(config.profile, DEFAULT_PROFILE);
        assert!(config.order_expiry_secs.is_none());
        assert!(errors_of(&config).is_empty());
    }

    #[test]
    fn cli_profile_is_known() {
        let config = MobileConfig {
            profile: CLI_PROFILE.into(),
            ..valid()
        };

        assert!(errors_of(&config).is_empty(), "{:?}", errors_of(&config));
    }

    #[test]
    fn unknown_profile_is_rejected() {
        let mut config = valid();
        config.profile = "desktop".into();

        assert!(
            errors_of(&config)
                .iter()
                .any(|e| e.contains("mobile.profile"))
        );
    }

    #[test]
    fn missing_source_selects_managed_default() {
        let mut config = valid();
        config.source_dir.clear();

        assert!(errors_of(&config).is_empty());
        config.app_ref = Some("main".into());
        config.source_dir = "../mobilev2".into();
        assert!(
            errors_of(&config)
                .iter()
                .any(|e| e.contains("mutually exclusive"))
        );
    }

    #[test]
    fn invalid_pubkey_is_reported() {
        let mut config = valid();
        config.mostro_pubkey = "not-a-key".into();

        let errors = errors_of(&config);

        assert_eq!(errors.len(), 1, "{errors:?}");
        assert!(config.mostro_pubkey().is_none());
    }

    #[test]
    fn unknown_fields_are_rejected() {
        let result: Result<MobileConfig, _> =
            toml::from_str(&format!("mostro_pubkey = \"{HEX}\"\nbogus = 1"));

        assert!(result.is_err());
    }
}
