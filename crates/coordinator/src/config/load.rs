//! Loading: TOML baseline, optional local overlay, `env:` resolution and the
//! rule that secrets never appear literally in TOML.

use std::path::{Path, PathBuf};

use figment::Figment;
use figment::providers::{Format, Toml};
use mortsom_common::env_resolve::{ENV_PREFIX, MissingEnv, resolve_env_lenient, resolve_env_with};
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

/// The `mobile.profile` a configuration selects, read without loading it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfiguredProfile {
    /// The files cannot be read, set no profile, or set one that is not a
    /// string (the loader refuses that): the default applies.
    Absent,
    /// The profile, its own environment reference resolved.
    Named(String),
    /// The profile references environment variables that are not set, so
    /// it names no profile; the loader refuses such a configuration.
    Unresolved {
        /// The names that are not set; never a value.
        names: Vec<String>,
    },
}

impl ConfiguredProfile {
    /// The profile an invocation drives: `flag` (`--profile`) when given,
    /// else this one, else the default. `Err` holds the names an
    /// unresolved profile needs, when no flag overrides it.
    pub fn with_flag(self, flag: Option<&str>) -> Result<String, Vec<String>> {
        match (flag, self) {
            (Some(flag), _) => Ok(flag.to_owned()),
            (None, Self::Named(profile)) => Ok(profile),
            (None, Self::Absent) => Ok(mortsom_app_contract::config::DEFAULT_PROFILE.to_owned()),
            (None, Self::Unresolved { names }) => Err(names),
        }
    }
}

/// The `mobile.profile` the configuration at `path` selects, its local
/// overlay included and its own environment reference resolved from the
/// process environment, without resolving secrets or validating anything
/// else.
pub fn configured_profile(path: &Path) -> ConfiguredProfile {
    configured_profile_with(path, |name| std::env::var(name).ok())
}

/// Like [`configured_profile`] with an injectable environment lookup. The
/// profile's own `env:` reference or `${NAME}` is resolved, as the loader
/// resolves it; a name it references that is not set leaves it
/// [`ConfiguredProfile::Unresolved`], never absent. A name set to an empty
/// value counts as unset whatever `lookup` returns, as doctor and run read
/// the environment (`RealHost::env`), so every caller agrees on the client.
pub fn configured_profile_with<F>(path: &Path, lookup: F) -> ConfiguredProfile
where
    F: Fn(&str) -> Option<String>,
{
    let Some(profile) = read_layered(path)
        .ok()
        .and_then(|raw| raw.get("mobile")?.get("profile").cloned())
    else {
        return ConfiguredProfile::Absent;
    };
    match resolve_env_with(profile, |name| lookup(name).filter(|v| !v.is_empty())) {
        Ok(value) => value.as_str().map_or(ConfiguredProfile::Absent, |profile| {
            ConfiguredProfile::Named(profile.to_owned())
        }),
        Err(MissingEnv(names)) => ConfiguredProfile::Unresolved { names },
    }
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

    fn named(profile: &str) -> ConfiguredProfile {
        ConfiguredProfile::Named(profile.to_owned())
    }

    #[test]
    fn the_configured_profile_reads_the_local_overlay_too() {
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path().join("desktop.toml");
        std::fs::write(&base, "[mobile]\nprofile = \"mobile-v2\"\n").unwrap();
        assert_eq!(configured_profile(&base), named("mobile-v2"));

        std::fs::write(
            dir.path().join("desktop.local.toml"),
            "[mobile]\nprofile = \"cli\"\n",
        )
        .unwrap();
        assert_eq!(configured_profile(&base), named("cli"));

        assert_eq!(
            configured_profile(&dir.path().join("missing.toml")),
            ConfiguredProfile::Absent
        );
        std::fs::write(&base, "[run]\n").unwrap();
        std::fs::remove_file(dir.path().join("desktop.local.toml")).unwrap();
        assert_eq!(configured_profile(&base), ConfiguredProfile::Absent);
    }

    /// A reference to a set name is resolved as the loader resolves it, so
    /// `run`, `doctor` and the loaded configuration agree on the profile.
    /// An unset name leaves it unresolved, where the loader fails: it is
    /// not the absent profile, whose default would name a client.
    #[test]
    fn the_configured_profile_resolves_its_environment_reference() {
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path().join("desktop.toml");
        let lookup = |name: &str| (name == "MORTSOM_PROFILE").then(|| "cli".to_owned());
        for reference in ["env:MORTSOM_PROFILE", "${MORTSOM_PROFILE}"] {
            std::fs::write(&base, format!("[mobile]\nprofile = \"{reference}\"\n")).unwrap();

            assert_eq!(
                configured_profile_with(&base, lookup),
                named("cli"),
                "{reference}"
            );
        }

        for reference in ["env:MORTSOM_UNSET", "${MORTSOM_UNSET}"] {
            std::fs::write(&base, format!("[mobile]\nprofile = \"{reference}\"\n")).unwrap();

            assert_eq!(
                configured_profile_with(&base, lookup),
                ConfiguredProfile::Unresolved {
                    names: vec!["MORTSOM_UNSET".to_owned()]
                },
                "{reference}"
            );
        }
    }

    /// A name set to an empty value is unset, as doctor and run read the
    /// environment (`RealHost::env`): an empty profile would read as the
    /// app's, naming a client neither of them checks or drives.
    #[test]
    fn a_profile_reference_set_to_an_empty_value_is_unresolved() {
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path().join("desktop.toml");
        let empty = |_: &str| Some(String::new());
        for reference in ["env:MORTSOM_PROFILE", "${MORTSOM_PROFILE}"] {
            std::fs::write(&base, format!("[mobile]\nprofile = \"{reference}\"\n")).unwrap();

            assert_eq!(
                configured_profile_with(&base, empty),
                ConfiguredProfile::Unresolved {
                    names: vec!["MORTSOM_PROFILE".to_owned()]
                },
                "{reference}"
            );
        }
    }

    /// `--profile` wins over whatever the configuration says, an unset
    /// reference included; without it, an absent profile is the default
    /// and an unresolved one names no profile, only the unset names.
    #[test]
    fn the_profile_flag_wins_and_an_unresolved_profile_names_its_variables() {
        let unresolved = || ConfiguredProfile::Unresolved {
            names: vec!["MORTSOM_PROFILE".to_owned()],
        };

        assert_eq!(unresolved().with_flag(Some("cli")), Ok("cli".to_owned()));
        assert_eq!(
            named("cli").with_flag(Some("mobile-v2")).unwrap(),
            "mobile-v2"
        );
        assert_eq!(named("cli").with_flag(None).unwrap(), "cli");
        assert_eq!(
            ConfiguredProfile::Absent.with_flag(None).unwrap(),
            mortsom_app_contract::config::DEFAULT_PROFILE
        );
        assert_eq!(
            unresolved().with_flag(None),
            Err(vec!["MORTSOM_PROFILE".to_owned()])
        );
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
