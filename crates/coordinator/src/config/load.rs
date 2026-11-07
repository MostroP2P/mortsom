//! Loading: TOML baseline, optional local overlay, `env:` resolution and the
//! rule that secrets never appear literally in TOML.

use std::path::{Path, PathBuf};

use figment::Figment;
use figment::providers::{Format, Toml};
use mortsom_common::env_resolve::{ENV_PREFIX, resolve_env_lenient, resolve_env_with};
use mortsom_environment::config::ENV_ONLY_KEYS;
use toml::Value;

use super::{AppSource, Config, ConfigError};

/// Suffix of the gitignored overlay merged on top of the baseline file:
/// `mortsom.toml` + `mortsom.local.toml`.
pub const LOCAL_OVERLAY_SUFFIX: &str = ".local.toml";

/// Result of a lenient load used by diagnostics such as `mortsom doctor`.
#[derive(Debug)]
pub struct ConfigDiagnosis {
    /// Parsed configuration with empty strings where secrets were missing,
    /// or `None` when the TOML itself could not be parsed.
    pub config: Option<Config>,
    /// Environment variables referenced but not set.
    pub missing_env: Vec<String>,
    /// Every other problem: parse errors, secrets in TOML, validation.
    pub problems: Vec<String>,
}

impl Config {
    /// Loads, resolves and validates the configuration from `path` plus its
    /// local overlay, reading secrets from the process environment.
    pub fn load(path: &Path) -> Result<Self, ConfigError> {
        Self::load_with(path, |name| std::env::var(name).ok())
    }

    /// Like [`Config::load`] with an injectable environment lookup.
    pub fn load_with<F>(path: &Path, lookup: F) -> Result<Self, ConfigError>
    where
        F: Fn(&str) -> Option<String>,
    {
        Self::load_with_source(path, lookup, None)
    }

    /// Loads and validates after replacing the configured source mode, if selected.
    pub fn load_with_source<F>(
        path: &Path,
        lookup: F,
        source: Option<&AppSource>,
    ) -> Result<Self, ConfigError>
    where
        F: Fn(&str) -> Option<String>,
    {
        let raw = read_layered(path)?;
        Self::from_toml_value(raw, lookup, source)
    }

    /// Parses a complete configuration from TOML text; the injectable path
    /// used by tests.
    pub fn from_toml_str<F>(text: &str, lookup: F) -> Result<Self, ConfigError>
    where
        F: Fn(&str) -> Option<String>,
    {
        let raw: Value = toml::from_str(text).map_err(|e| ConfigError::Parse(e.to_string()))?;
        Self::from_toml_value(raw, lookup, None)
    }

    fn from_toml_value<F>(
        mut raw: Value,
        lookup: F,
        source: Option<&AppSource>,
    ) -> Result<Self, ConfigError>
    where
        F: Fn(&str) -> Option<String>,
    {
        reject_literal_secrets(&raw)?;
        if let Some(source) = source {
            source.remove_configured(&mut raw);
        }
        let resolved = resolve_env_with(raw, lookup)?;
        let mut config: Config = resolved
            .try_into()
            .map_err(|e: toml::de::Error| ConfigError::Parse(e.to_string()))?;
        if let Some(source) = source {
            source.apply(&mut config);
        }
        config.validate()?;
        Ok(config)
    }

    /// Loads without failing closed, collecting every problem for a report.
    /// Never use the returned configuration to start anything.
    pub fn diagnose<F>(path: &Path, lookup: F) -> ConfigDiagnosis
    where
        F: Fn(&str) -> Option<String>,
    {
        Self::diagnose_with_source(path, lookup, None)
    }

    /// Diagnoses the effective source selection, retaining every unrelated problem.
    /// Never use the returned configuration to start anything.
    pub fn diagnose_with_source<F>(
        path: &Path,
        lookup: F,
        source: Option<&AppSource>,
    ) -> ConfigDiagnosis
    where
        F: Fn(&str) -> Option<String>,
    {
        let mut raw = match read_layered(path) {
            Ok(raw) => raw,
            Err(e) => {
                return ConfigDiagnosis {
                    config: None,
                    missing_env: Vec::new(),
                    problems: vec![e.to_string()],
                };
            }
        };
        let mut problems = Vec::new();
        if let Err(e) = reject_literal_secrets(&raw) {
            problems.push(e.to_string());
        }
        if let Some(source) = source {
            source.remove_configured(&mut raw);
        }
        let (resolved, missing_env) = resolve_env_lenient(raw, lookup);
        let config = match resolved.try_into::<Config>() {
            Ok(mut config) => {
                if let Some(source) = source {
                    source.apply(&mut config);
                }
                if let Err(ConfigError::Invalid(errors)) = config.validate() {
                    problems.extend(errors);
                }
                Some(config)
            }
            Err(e) => {
                problems.push(format!("invalid config: {e}"));
                None
            }
        };
        ConfigDiagnosis {
            config,
            missing_env,
            problems,
        }
    }
}

/// Path of the local overlay for a baseline file.
pub fn local_overlay_path(path: &Path) -> PathBuf {
    let stem = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("mortsom");
    path.with_file_name(format!("{stem}{LOCAL_OVERLAY_SUFFIX}"))
}

/// The `mobile.profile` the configuration at `path` selects, its local
/// overlay included and its own environment reference resolved from the
/// process environment, without resolving secrets or validating anything
/// else. `None` when the files cannot be read, no profile is set, or a name
/// it references is not set.
pub fn configured_profile(path: &Path) -> Option<String> {
    configured_profile_with(path, |name| std::env::var(name).ok())
}

/// Like [`configured_profile`] with an injectable environment lookup. The
/// profile's own `env:` reference or `${NAME}` is resolved, as the loader
/// resolves it; `None` when a name it references is not set.
pub fn configured_profile_with<F>(path: &Path, lookup: F) -> Option<String>
where
    F: Fn(&str) -> Option<String>,
{
    let profile = read_layered(path)
        .ok()?
        .get("mobile")?
        .get("profile")?
        .clone();
    resolve_env_with(profile, lookup)
        .ok()?
        .as_str()
        .map(str::to_owned)
}

pub(super) fn read_layered(path: &Path) -> Result<Value, ConfigError> {
    if !path.exists() {
        return Err(ConfigError::Read {
            path: path.display().to_string(),
            source: std::io::Error::new(std::io::ErrorKind::NotFound, "no such file"),
        });
    }
    let mut figment = Figment::new().merge(Toml::file(path));
    let overlay = local_overlay_path(path);
    if overlay.exists() {
        figment = figment.merge(Toml::file(&overlay));
    }
    figment
        .extract::<Value>()
        .map_err(|e| ConfigError::Parse(e.to_string()))
}

pub(super) fn reject_literal_secrets(raw: &Value) -> Result<(), ConfigError> {
    for key in ENV_ONLY_KEYS {
        let Some(value) = lookup_dotted(raw, key) else {
            continue;
        };
        let is_reference = value
            .as_str()
            .is_some_and(|text| text.starts_with(ENV_PREFIX));
        if !is_reference {
            return Err(ConfigError::SecretInToml {
                key: (*key).to_owned(),
            });
        }
    }
    Ok(())
}

fn lookup_dotted<'a>(value: &'a Value, dotted: &str) -> Option<&'a Value> {
    dotted
        .split('.')
        .try_fold(value, |current, part| current.get(part))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use mortsom_common::Lane;

    const HEX: &str = "dbe0b1be7aafd3cfba92d7463edbd4e33b2969f61bd554d37ac56f032e13355a";
    const ALICE: &str = "nostr+walletconnect://b889ff5b1513b641e2a139f661a661364979c5beee91842f8f0ef42ab558e9d4?relay=wss%3A%2F%2Frelay.damus.io&secret=71a8c14c1407c113601079c4302dab36460f0ccd0ad506f1f2dc73b5100e4f3c";
    const BOB: &str = "nostr+walletconnect://b889ff5b1513b641e2a139f661a661364979c5beee91842f8f0ef42ab558e9d4?relay=wss%3A%2F%2Frelay.damus.io&secret=0000000000000000000000000000000000000000000000000000000000000001";

    fn lookup(name: &str) -> Option<String> {
        match name {
            "MORTSOM_MOSTRO_PUBKEY" => Some(HEX.into()),
            "MORTSOM_MOSTRO_SETTINGS_FILE" => Some("/tmp/settings.toml".into()),
            "MORTSOM_ALICE_NWC" => Some(ALICE.into()),
            "MORTSOM_BOB_NWC" => Some(BOB.into()),
            _ => None,
        }
    }

    fn example() -> String {
        std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../mortsom.toml.example"
        ))
        .unwrap()
    }

    #[test]
    fn an_android_section_is_no_longer_accepted() {
        let text = format!(
            "{}
[android]
avd_image = \"x\"
",
            example()
        );
        assert!(matches!(
            Config::from_toml_str(&text, lookup),
            Err(ConfigError::Parse(_))
        ));
    }

    #[test]
    fn shared_deadline_has_a_default_overrides_and_rejects_zero() {
        let mut value: Value = toml::from_str(&example()).unwrap();
        value.as_table_mut().unwrap().remove("ui");
        let config = Config::from_toml_str(&toml::to_string(&value).unwrap(), lookup).unwrap();
        assert_eq!(
            config.ui_timeout_secs(),
            super::super::DEFAULT_UI_TIMEOUT_SECS
        );
        value.as_table_mut().unwrap().insert(
            "ui".into(),
            toml::Value::Table(toml::Table::from_iter([(
                "timeout_secs".into(),
                Value::Integer(45),
            )])),
        );
        let config = Config::from_toml_str(&toml::to_string(&value).unwrap(), lookup).unwrap();
        assert_eq!(config.ui_timeout_secs(), 45);
        value["ui"]["timeout_secs"] = Value::Integer(0);
        assert!(matches!(
            Config::from_toml_str(&toml::to_string(&value).unwrap(), lookup),
            Err(ConfigError::Invalid(_))
        ));
    }

    #[test]
    fn committed_example_loads_with_all_secrets_present() {
        let config = Config::from_toml_str(&example(), lookup).unwrap();

        assert_eq!(config.run.lane, Lane::Local);
        assert_eq!(config.mobile.mostro_pubkey, HEX);
        assert_eq!(config.mobile.profile, "mobile-v2");
        assert!(matches!(
            &config.wallets.alice,
            mortsom_environment::WalletConfig::Nwc { uri } if uri.expose() == ALICE
        ));
        assert!(!config.run.allow_mainnet);
    }

    #[test]
    fn every_missing_secret_is_reported_at_once() {
        let err = Config::from_toml_str(&example(), |_| None).unwrap_err();

        match err {
            ConfigError::MissingEnv(missing) => assert_eq!(
                missing.0,
                vec![
                    "MORTSOM_ALICE_NWC",
                    "MORTSOM_BOB_NWC",
                    "MORTSOM_MOSTRO_PUBKEY",
                    "MORTSOM_MOSTRO_SETTINGS_FILE",
                ]
            ),
            other => panic!("expected MissingEnv, got {other:?}"),
        }
    }

    #[test]
    fn literal_secret_in_toml_is_rejected_before_resolution() {
        let text = example().replace("env:MORTSOM_BOB_NWC", BOB);

        let err = Config::from_toml_str(&text, lookup).unwrap_err();

        assert!(matches!(err, ConfigError::SecretInToml { ref key } if key == "wallets.bob.uri"));
        assert!(!err.to_string().contains("secret="));
    }

    #[test]
    fn literal_settings_path_in_toml_is_rejected() {
        let text = example().replace("env:MORTSOM_MOSTRO_SETTINGS_FILE", "/home/me/settings.toml");

        let err = Config::from_toml_str(&text, lookup).unwrap_err();

        assert!(
            matches!(err, ConfigError::SecretInToml { ref key } if key == "mostro.settings_file")
        );
    }

    #[test]
    fn validation_problems_are_aggregated() {
        let text = example()
            .replace("amount_sats = 1000", "amount_sats = 0")
            .replace("jobs = 1", "jobs = 0");

        let err = Config::from_toml_str(&text, lookup).unwrap_err();

        match err {
            ConfigError::Invalid(errors) => assert!(errors.len() >= 2, "{errors:?}"),
            other => panic!("expected Invalid, got {other:?}"),
        }
    }

    #[test]
    fn unknown_top_level_section_is_a_parse_error() {
        let text = format!("{}\n[extra]\nx = 1\n", example());

        let err = Config::from_toml_str(&text, lookup).unwrap_err();

        assert!(matches!(err, ConfigError::Parse(_)));
    }

    #[test]
    fn local_overlay_is_merged_on_top_of_baseline() {
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path().join("mortsom.toml");
        std::fs::write(&base, example()).unwrap();
        std::fs::write(
            local_overlay_path(&base),
            "[run]\nlane = \"ci\"\namount_sats = 1000\nmax_order_sats = 5000\nmax_run_spend_sats = 20000\n",
        )
        .unwrap();

        let config = Config::load_with(&base, lookup).unwrap();

        assert_eq!(config.run.lane, Lane::Ci);
        assert_eq!(
            config.relay.host_port, 7500,
            "baseline values survive the overlay"
        );
    }

    #[test]
    fn missing_file_is_a_read_error() {
        let err = Config::load_with(Path::new("/nonexistent/mortsom.toml"), lookup).unwrap_err();

        assert!(matches!(err, ConfigError::Read { .. }));
    }

    #[test]
    fn diagnose_reports_missing_env_and_validation_without_failing() {
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path().join("mortsom.toml");
        std::fs::write(&base, example()).unwrap();

        let diagnosis = Config::diagnose(&base, |name| match name {
            "MORTSOM_MOSTRO_PUBKEY" => Some(HEX.into()),
            _ => None,
        });

        assert_eq!(diagnosis.missing_env.len(), 3);
        let config = diagnosis.config.expect("config parses with placeholders");
        assert!(matches!(
            &config.wallets.alice,
            mortsom_environment::WalletConfig::Nwc { uri } if uri.is_empty()
        ));
        assert!(
            diagnosis
                .problems
                .iter()
                .any(|p| p.contains("unresolved secret"))
        );
    }

    #[test]
    fn diagnose_reports_unreadable_file() {
        let diagnosis = Config::diagnose(Path::new("/nonexistent/x.toml"), lookup);

        assert!(diagnosis.config.is_none());
        assert_eq!(diagnosis.problems.len(), 1);
    }

    #[test]
    fn diagnose_reports_literal_secret_and_parse_failure() {
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path().join("mortsom.toml");
        std::fs::write(
            &base,
            "[wallets.alice]\nkind = \"nwc\"\nuri = \"nostr+walletconnect://x\"\n",
        )
        .unwrap();

        let diagnosis = Config::diagnose(&base, lookup);

        assert!(diagnosis.config.is_none());
        assert!(
            diagnosis
                .problems
                .iter()
                .any(|p| p.contains("wallets.alice.uri"))
        );
        assert!(
            diagnosis
                .problems
                .iter()
                .any(|p| p.contains("invalid config"))
        );
    }

    #[test]
    fn overlay_path_sits_next_to_baseline() {
        assert_eq!(
            local_overlay_path(Path::new("/x/mortsom.toml")),
            PathBuf::from("/x/mortsom.local.toml")
        );
        assert_eq!(
            local_overlay_path(Path::new("config/ci.example.toml")),
            PathBuf::from("config/ci.example.local.toml")
        );
    }

    #[test]
    fn the_configured_profile_reads_the_local_overlay_too() {
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path().join("desktop.toml");
        std::fs::write(&base, "[mobile]\nprofile = \"mobile-v2\"\n").unwrap();
        assert_eq!(configured_profile(&base).as_deref(), Some("mobile-v2"));

        std::fs::write(
            dir.path().join("desktop.local.toml"),
            "[mobile]\nprofile = \"cli\"\n",
        )
        .unwrap();
        assert_eq!(configured_profile(&base).as_deref(), Some("cli"));

        assert_eq!(configured_profile(&dir.path().join("missing.toml")), None);
        std::fs::write(&base, "[run]\n").unwrap();
        std::fs::remove_file(dir.path().join("desktop.local.toml")).unwrap();
        assert_eq!(configured_profile(&base), None);
    }

    /// A reference to a set name is resolved as the loader resolves it, so
    /// `run`, `doctor` and the loaded configuration agree on the profile.
    /// An unset name gives no profile, where the loader fails.
    #[test]
    fn the_configured_profile_resolves_its_environment_reference() {
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path().join("desktop.toml");
        let lookup = |name: &str| (name == "MORTSOM_PROFILE").then(|| "cli".to_owned());
        for reference in ["env:MORTSOM_PROFILE", "${MORTSOM_PROFILE}"] {
            std::fs::write(&base, format!("[mobile]\nprofile = \"{reference}\"\n")).unwrap();

            assert_eq!(
                configured_profile_with(&base, lookup).as_deref(),
                Some("cli"),
                "{reference}"
            );
        }

        std::fs::write(&base, "[mobile]\nprofile = \"env:MORTSOM_UNSET\"\n").unwrap();
        assert_eq!(configured_profile_with(&base, lookup), None);
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod lane_examples {
    use super::*;
    use mortsom_common::Lane;

    fn lookup(name: &str) -> Option<String> {
        match name {
            "MORTSOM_MOSTRO_PUBKEY" => {
                Some("dbe0b1be7aafd3cfba92d7463edbd4e33b2969f61bd554d37ac56f032e13355a".into())
            }
            "MORTSOM_MOSTRO_SETTINGS_FILE" => Some("/tmp/settings.toml".into()),
            "MORTSOM_ARTIFACTS_DIR" => Some("/tmp/artifacts".into()),
            "MORTSOM_ALICE_NWC" => Some("nostr+walletconnect://b889ff5b1513b641e2a139f661a661364979c5beee91842f8f0ef42ab558e9d4?relay=wss%3A%2F%2Frelay.damus.io&secret=71a8c14c1407c113601079c4302dab36460f0ccd0ad506f1f2dc73b5100e4f3c".into()),
            "MORTSOM_BOB_NWC" => Some("nostr+walletconnect://b889ff5b1513b641e2a139f661a661364979c5beee91842f8f0ef42ab558e9d4?relay=wss%3A%2F%2Frelay.damus.io&secret=0000000000000000000000000000000000000000000000000000000000000001".into()),
            _ => None,
        }
    }

    fn load(name: &str) -> Config {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../config")
            .join(name);
        Config::load_with(&path, lookup).unwrap()
    }

    #[test]
    fn local_example_is_the_local_lane() {
        assert_eq!(load("local.example.toml").run.lane, Lane::Local);
    }

    #[test]
    fn ci_example_is_the_ci_lane_with_artifacts_from_the_environment() {
        let config = load("ci.example.toml");
        assert_eq!(config.run.lane, Lane::Ci);
        assert_eq!(config.run.artifacts_dir, "/tmp/artifacts");
    }

    #[test]
    fn canary_example_opts_into_mainnet_with_tiny_caps() {
        let config = load("canary.example.toml");
        assert_eq!(config.run.lane, Lane::Canary);
        assert!(config.run.allow_mainnet);
        assert!(config.run.max_run_spend_sats <= 3000);
    }

    #[test]
    fn every_example_drives_the_v2_app_from_its_checkout() {
        for name in [
            "local.example.toml",
            "ci.example.toml",
            "canary.example.toml",
        ] {
            let config = load(name);
            assert_eq!(config.mobile.profile, "mobile-v2", "{name}");
            assert_eq!(config.mobile.source_dir, "../mobilev2", "{name}");
        }
    }
    #[test]
    fn remote_selection_validates_and_cli_overrides_the_configured_source_mode() {
        use crate::{app_source::Selector, cli::SourceOverrides};
        let mut config = load("local.example.toml");
        let remote = SourceOverrides {
            app_ref: Some(Selector::parse("feature/fix").unwrap()),
            app_source: None,
        };
        remote.selection().unwrap().apply(&mut config);
        assert!(config.mobile.source_dir.is_empty());
        assert_eq!(config.mobile.app_ref.as_deref(), Some("feature/fix"));
        assert!(config.validate().is_ok());
        SourceOverrides {
            app_ref: None,
            app_source: Some("/developer/app".into()),
        }
        .selection()
        .unwrap()
        .apply(&mut config);
        assert_eq!(config.mobile.source_dir, "/developer/app");
        assert!(config.mobile.app_ref.is_none());
        config.mobile.app_ref = Some("main".into());
        assert!(
            config.validate().is_err(),
            "configured source modes conflict"
        );
        config.mobile.source_dir.clear();
        config.mobile.app_ref = Some("https://github.com/other/app/pull/1".into());
        assert!(
            config.validate().is_err(),
            "config selectors use the CLI validation"
        );
        config.mobile.app_ref = None;
        assert!(
            config.validate().is_ok(),
            "omitted source selects managed main"
        );
    }
}
