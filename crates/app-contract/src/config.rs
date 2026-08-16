//! The `[mobile]` configuration section: which application build Mortsom
//! drives and how it is produced.

use std::str::FromStr;

use nostr::key::PublicKey;
use serde::{Deserialize, Serialize};

/// Default application profile (spec §12.2).
pub const DEFAULT_PROFILE: &str = "mobile-legacy";
/// Profiles Mortsom knows how to drive.
pub const KNOWN_PROFILES: &[&str] = &["mobile-legacy", "mobile-v2"];

/// `[mobile]` section.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MobileConfig {
    /// Named application profile: `mobile-legacy` or `mobile-v2`.
    #[serde(default = "default_profile")]
    pub profile: String,
    /// Source checkout used to build the APK when `apk_path` is empty.
    #[serde(default)]
    pub source_dir: String,
    /// Build flavor; the Mortsom-only flavor disables production relay fallback.
    #[serde(default = "default_flavor")]
    pub flavor: String,
    /// Prebuilt APK path; empty means build from `source_dir`.
    #[serde(default)]
    pub apk_path: String,
    /// Public key of the daemon under test (hex or npub). Not a secret.
    pub mostro_pubkey: String,
    /// Extra `--dart-define` values passed to the Flutter build. Never secrets.
    #[serde(default)]
    pub dart_defines: Vec<String>,
    /// Android application id of the build under test.
    #[serde(default = "default_package")]
    pub package: String,
    /// Launch activity; empty lets UiAutomator2 resolve the launcher.
    #[serde(default)]
    pub main_activity: String,
}

fn default_package() -> String {
    "network.mostro.app".to_owned()
}

fn default_profile() -> String {
    DEFAULT_PROFILE.to_owned()
}

fn default_flavor() -> String {
    "mortsom".to_owned()
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
        // The two applications ship under different ids; the default
        // package belongs to the classic app.
        if self.profile == "mobile-v2" && self.package == default_package() {
            errors.push(
                "mobile.profile = mobile-v2 needs mobile.package set to the v2 application id (foundation.mostro.app)"
                    .to_owned(),
            );
        }
        if self.source_dir.is_empty() && self.apk_path.is_empty() {
            errors.push("mobile: either source_dir or apk_path must be set".to_owned());
        }
        if self.flavor.is_empty() {
            errors.push("mobile.flavor must not be empty".to_owned());
        }
        if PublicKey::from_str(&self.mostro_pubkey).is_err() {
            errors.push("mobile.mostro_pubkey is not a valid hex or npub public key".to_owned());
        }
        if self.package.is_empty() || !self.package.contains('.') {
            errors.push("mobile.package must be a dotted Android application id".to_owned());
        }
        for define in &self.dart_defines {
            if !define.contains('=') {
                errors.push(format!(
                    "mobile.dart_defines entry `{define}` must be KEY=VALUE"
                ));
            }
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

    const PUBKEY: &str = "npub1qqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqq";
    const HEX: &str = "dbe0b1be7aafd3cfba92d7463edbd4e33b2969f61bd554d37ac56f032e13355a";

    fn valid() -> MobileConfig {
        MobileConfig {
            profile: DEFAULT_PROFILE.into(),
            source_dir: "../mobile".into(),
            flavor: "mortsom".into(),
            apk_path: String::new(),
            mostro_pubkey: HEX.into(),
            dart_defines: vec![format!("MOSTRO_PUB_KEY={HEX}")],
            package: "network.mostro.app".into(),
            main_activity: String::new(),
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
            "mostro_pubkey = \"{HEX}\"\napk_path = \"app.apk\""
        ))
        .unwrap();

        assert_eq!(config.profile, DEFAULT_PROFILE);
        assert_eq!(config.flavor, "mortsom");
        assert_eq!(config.package, "network.mostro.app");
        assert!(errors_of(&config).is_empty());
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
    fn v2_profile_needs_the_v2_package() {
        let mut config = valid();
        config.profile = "mobile-v2".into();
        assert!(errors_of(&config)[0].contains("foundation.mostro.app"));
        config.package = "foundation.mostro.app".into();
        assert!(errors_of(&config).is_empty());
    }

    #[test]
    fn missing_source_and_apk_is_rejected() {
        let mut config = valid();
        config.source_dir.clear();

        assert!(
            errors_of(&config)
                .iter()
                .any(|e| e.contains("source_dir or apk_path"))
        );
    }

    #[test]
    fn invalid_pubkey_and_bad_define_are_both_reported() {
        let mut config = valid();
        config.mostro_pubkey = "not-a-key".into();
        config.dart_defines.push("NOEQUALS".into());
        config.flavor.clear();

        let errors = errors_of(&config);

        assert_eq!(errors.len(), 3, "{errors:?}");
        assert!(config.mostro_pubkey().is_none());
        let _ = PUBKEY;
    }

    #[test]
    fn unknown_fields_are_rejected() {
        let result: Result<MobileConfig, _> =
            toml::from_str(&format!("mostro_pubkey = \"{HEX}\"\nbogus = 1"));

        assert!(result.is_err());
    }
}
