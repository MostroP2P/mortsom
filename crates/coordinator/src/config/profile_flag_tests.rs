//! `--profile` in place of `mobile.profile`, replaced before the
//! configuration is resolved and validated, in the strict loader and in
//! the lenient diagnosis.
use super::tests::{ALICE, example, lookup};
use super::*;
use mortsom_app_contract::config::KNOWN_PROFILES;

/// A reference to a name [`lookup`] does not set.
const UNSET: &str = "env:MORTSOM_PROFILE";

/// The example configuration, with `mobile.profile = <profile>` in its
/// local overlay.
fn configured(profile: &str) -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("mortsom.toml");
    std::fs::write(&path, example()).unwrap();
    std::fs::write(
        dir.path().join("mortsom.local.toml"),
        format!("[mobile]\nprofile = \"{profile}\"\n"),
    )
    .unwrap();
    (dir, path)
}

/// [`lookup`] with `MORTSOM_PROFILE` set to an empty value.
fn empty_profile(name: &str) -> Option<String> {
    match name {
        "MORTSOM_PROFILE" => Some(String::new()),
        _ => lookup(name),
    }
}

#[test]
fn a_known_flag_loads_over_an_unset_reference_or_an_unknown_profile() {
    for configured_profile in [UNSET, "mobile-v1", "cli", "mobile-v2"] {
        let (_dir, path) = configured(configured_profile);
        for flag in KNOWN_PROFILES {
            let config = Config::load_with_source(&path, lookup, None, Some(flag))
                .unwrap_or_else(|e| panic!("{configured_profile} / {flag}: {e}"));

            assert_eq!(config.mobile.profile, *flag, "{configured_profile}");
        }
    }
}

#[test]
fn a_known_flag_loads_over_a_reference_set_to_an_empty_value() {
    let (_dir, path) = configured(UNSET);

    let config = Config::load_with_source(&path, empty_profile, None, Some("cli")).unwrap();

    assert_eq!(config.mobile.profile, "cli");
}

#[test]
fn without_the_flag_the_configured_profile_is_refused_as_before() {
    let (_dir, path) = configured(UNSET);
    match Config::load_with_source(&path, lookup, None, None) {
        Err(ConfigError::MissingEnv(MissingEnv(names))) => {
            assert_eq!(names, ["MORTSOM_PROFILE"]);
        }
        other => panic!("expected the unset reference: {other:?}"),
    }
    let (_dir, path) = configured("mobile-v1");
    match Config::load_with_source(&path, lookup, None, None) {
        Err(ConfigError::Invalid(errors)) => {
            assert!(
                errors
                    .iter()
                    .any(|e| e.contains("mobile.profile `mobile-v1`")),
                "{errors:?}"
            );
        }
        other => panic!("expected the unknown profile: {other:?}"),
    }
}

/// An unknown flag replaces nothing: the configuration loads or fails as
/// without it, and the client resolution refuses the flag by name.
#[test]
fn an_unknown_flag_is_never_written_into_the_configuration() {
    let (_dir, path) = configured("cli");
    let config = Config::load_with_source(&path, lookup, None, Some("mobile-v1")).unwrap();
    assert_eq!(config.mobile.profile, "cli");

    let (_dir, path) = configured(UNSET);
    assert!(matches!(
        Config::load_with_source(&path, lookup, None, Some("mobile-v1")),
        Err(ConfigError::MissingEnv(_))
    ));
    let diagnosis = Config::diagnose_with_source(&path, lookup, None, Some("mobile-v1"));
    assert_eq!(diagnosis.missing_env, ["MORTSOM_PROFILE"]);
    assert!(
        diagnosis.problems.iter().all(|p| !p.contains("mobile-v1")),
        "{:?}",
        diagnosis.problems
    );
}

/// The flag replaces the profile only: a literal secret in the files is
/// still refused, and a source selection still applies.
#[test]
fn the_flag_keeps_the_secret_check_and_the_source_selection() {
    let (dir, path) = configured(UNSET);
    let selection = AppSource::Local("/developer/app".into());
    let config = Config::load_with_source(&path, lookup, Some(&selection), Some("cli")).unwrap();
    assert_eq!(config.mobile.profile, "cli");
    assert_eq!(config.mobile.source_dir, "/developer/app");

    std::fs::write(
        dir.path().join("mortsom.local.toml"),
        format!("[mobile]\nprofile = \"{UNSET}\"\n[wallets.alice]\nuri = \"{ALICE}\"\n"),
    )
    .unwrap();
    assert!(matches!(
        Config::load_with_source(&path, lookup, None, Some("cli")),
        Err(ConfigError::SecretInToml { .. })
    ));
}

#[test]
fn the_diagnosis_takes_the_flag_in_place_of_the_configured_profile() {
    for configured_profile in [UNSET, "mobile-v1"] {
        let (_dir, path) = configured(configured_profile);

        let diagnosis = Config::diagnose_with_source(&path, lookup, None, Some("cli"));

        assert!(diagnosis.missing_env.is_empty(), "{configured_profile}");
        assert!(
            diagnosis.problems.is_empty(),
            "{configured_profile}: {:?}",
            diagnosis.problems
        );
        assert_eq!(diagnosis.config.unwrap().mobile.profile, "cli");
    }
}

#[test]
fn without_the_flag_the_diagnosis_reports_the_configured_profile_as_before() {
    let (_dir, path) = configured(UNSET);
    let diagnosis = Config::diagnose(&path, lookup);
    assert_eq!(diagnosis.missing_env, ["MORTSOM_PROFILE"]);

    let (_dir, path) = configured("mobile-v1");
    let diagnosis = Config::diagnose(&path, lookup);
    assert!(
        diagnosis
            .problems
            .iter()
            .any(|p| p.contains("mobile.profile `mobile-v1`")),
        "{:?}",
        diagnosis.problems
    );
}
