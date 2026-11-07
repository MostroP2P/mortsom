//! The build of the client under test, chosen the same way for every
//! profile: `--artifact`, `--source` or `--ref` select the app (profile
//! `mobile-v2`) or mostro-cli (profile `cli`), whichever the profile drives.
use clap::Args;
use std::path::{Path, PathBuf};

use crate::mostro_cli_build::CliSelection;

/// Which build of the profile's client a run or a check uses. With none,
/// the profile's default: the app's configured source, or mostro-cli's
/// `main`.
#[derive(Debug, Clone, Default, Args)]
pub struct ClientBuild {
    /// Prebuilt client: the app's native executable or Web bundle
    /// (`mobile-v2`), or a mostro-cli executable (`cli`).
    #[arg(long, value_name = "PATH", conflicts_with_all = ["source", "git_ref"])]
    pub artifact: Option<PathBuf>,
    /// Local checkout of the profile's client, built with its uncommitted
    /// changes.
    #[arg(long, value_name = "PATH", conflicts_with = "git_ref", value_parser = local_source)]
    pub source: Option<PathBuf>,
    /// Branch, or pull request URL, of the profile's repository
    /// (https://github.com/MostroP2P/app/pull/123 or
    /// https://github.com/MostroP2P/mostro-cli/pull/123); its head commit
    /// is built.
    #[arg(long = "ref", value_name = "BRANCH_OR_PR_URL")]
    pub git_ref: Option<String>,
}

fn local_source(value: &str) -> Result<PathBuf, &'static str> {
    if value.is_empty() {
        Err("--source requires a non-empty local checkout path")
    } else {
        Ok(PathBuf::from(value))
    }
}

/// A [`ClientBuild`] resolved for one profile.
#[derive(Debug, Clone, Default)]
pub struct ClientSelection {
    /// The app's prebuilt executable or bundle (`mobile-v2`).
    pub artifact: Option<PathBuf>,
    /// The app's source, replacing the configured one (`mobile-v2`).
    pub source: SourceOverrides,
    /// The mostro-cli build (`cli`).
    pub cli: CliSelection,
}

impl ClientBuild {
    /// Resolves the selection for `profile`: `--ref` must name a branch or
    /// a pull request of that profile's repository.
    pub fn for_profile(&self, profile: &str) -> Result<ClientSelection, String> {
        let invalid = |error: crate::git_source::SourceError| format!("--ref: {error}");
        if profile == crate::mostro_cli::PROFILE {
            let cli_ref = self
                .git_ref
                .as_deref()
                .map(crate::mostro_cli_build::Selector::parse)
                .transpose()
                .map_err(invalid)?;
            return Ok(ClientSelection {
                cli: CliSelection {
                    cli_artifact: self.artifact.clone(),
                    cli_ref,
                    cli_source: self.source.clone(),
                },
                ..ClientSelection::default()
            });
        }
        let app_ref = self
            .git_ref
            .as_deref()
            .map(crate::app_source::Selector::parse)
            .transpose()
            .map_err(invalid)?;
        Ok(ClientSelection {
            artifact: self.artifact.clone(),
            source: SourceOverrides {
                app_ref,
                app_source: self.source.clone(),
            },
            cli: CliSelection::default(),
        })
    }
}

/// The profile whose build flags a `run` or `doctor` reads: `--profile`,
/// else the configuration's `mobile.profile` (local overlay and its own
/// environment reference included), else the default.
///
/// A configured reference to an unset name names no profile; the flags are
/// then read as the app's, as for an unknown profile. Nothing is built
/// from them: doctor reports that profile as its `client` failure and the
/// loader refuses the configuration.
pub fn effective_profile(config_path: &Path, flag: Option<&str>) -> String {
    crate::config::configured_profile(config_path)
        .with_flag(flag)
        .unwrap_or_else(|_unset| mortsom_app_contract::config::DEFAULT_PROFILE.to_owned())
}

/// The app's source, replacing the configured mode: a branch or pull
/// request, or a local checkout.
#[derive(Debug, Clone, Default)]
pub struct SourceOverrides {
    /// App branch or https://github.com/MostroP2P/app/pull/123 (head commit).
    pub app_ref: Option<crate::app_source::Selector>,
    /// Existing local app checkout, including intentional developer changes.
    pub app_source: Option<PathBuf>,
}

impl SourceOverrides {
    /// Typed selection passed into configuration loading before validation.
    pub fn selection(&self) -> Option<crate::config::AppSource> {
        self.app_ref
            .clone()
            .map(crate::config::AppSource::Remote)
            .or_else(|| self.app_source.clone().map(crate::config::AppSource::Local))
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use clap::Parser;

    #[test]
    fn the_three_selections_conflict_before_startup() {
        for command in ["run", "doctor"] {
            for args in [
                vec!["--ref", "main", "--artifact", "/bundle"],
                vec!["--source", "/app", "--artifact", "/bundle"],
                vec!["--ref", "main", "--source", "/app"],
                vec!["--source", ""],
            ] {
                let mut invocation = vec!["mortsom", command];
                invocation.extend(args);
                assert!(super::super::Cli::try_parse_from(invocation).is_err());
            }
            for args in [["--ref", "feature/test"], ["--artifact", "/bundle"]] {
                let mut invocation = vec!["mortsom", command];
                invocation.extend(args);
                assert!(super::super::Cli::try_parse_from(invocation).is_ok());
            }
        }
    }

    fn build(artifact: Option<&str>, source: Option<&str>, git_ref: Option<&str>) -> ClientBuild {
        ClientBuild {
            artifact: artifact.map(PathBuf::from),
            source: source.map(PathBuf::from),
            git_ref: git_ref.map(str::to_owned),
        }
    }

    #[test]
    fn the_app_profile_reads_the_selection_as_the_app() {
        let selected = build(
            None,
            None,
            Some("https://github.com/MostroP2P/app/pull/410"),
        )
        .for_profile("mobile-v2")
        .unwrap();
        assert!(matches!(
            selected.source.selection(),
            Some(crate::config::AppSource::Remote(_))
        ));
        assert!(selected.cli.is_empty());

        let selected = build(Some("/bundle"), None, None)
            .for_profile("mobile-v2")
            .unwrap();
        assert_eq!(selected.artifact, Some(PathBuf::from("/bundle")));
        assert!(selected.source.selection().is_none());
        assert!(selected.cli.is_empty());
    }

    #[test]
    fn the_cli_profile_reads_the_selection_as_mostro_cli() {
        let selected = build(None, None, Some("develop"))
            .for_profile("cli")
            .unwrap();
        assert_eq!(
            selected.cli.cli_ref.unwrap().git_ref(),
            "refs/heads/develop"
        );
        assert!(selected.artifact.is_none());
        assert!(selected.source.selection().is_none());

        let selected = build(None, Some("../mostro-cli"), None)
            .for_profile("cli")
            .unwrap();
        assert_eq!(
            selected.cli.cli_source,
            Some(PathBuf::from("../mostro-cli"))
        );
        let selected = build(Some("/bin/mostro-cli"), None, None)
            .for_profile("cli")
            .unwrap();
        assert_eq!(
            selected.cli.cli_artifact,
            Some(PathBuf::from("/bin/mostro-cli"))
        );
    }

    #[test]
    fn a_pull_request_of_the_other_repository_is_refused() {
        let app_pull = build(None, None, Some("https://github.com/MostroP2P/app/pull/1"));
        let cli_pull = build(
            None,
            None,
            Some("https://github.com/MostroP2P/mostro-cli/pull/2"),
        );
        let error = app_pull.for_profile("cli").unwrap_err();
        assert!(error.starts_with("--ref:"), "{error}");
        assert!(cli_pull.for_profile("mobile-v2").is_err());
        assert!(
            build(None, None, Some("https://github.com/wrong/app/pull/2"))
                .for_profile("mobile-v2")
                .is_err()
        );
    }

    #[test]
    fn the_profile_flag_wins_over_the_configuration() {
        let missing = Path::new("/nonexistent/desktop.toml");
        assert_eq!(effective_profile(missing, Some("cli")), "cli");
        assert_eq!(
            effective_profile(missing, None),
            mortsom_app_contract::config::DEFAULT_PROFILE
        );
    }

    /// An unset reference names no profile: the build flags are read as
    /// the app's, unless `--profile` names the client.
    #[test]
    fn an_unresolved_profile_reads_the_build_flags_as_the_app_s() {
        let dir = tempfile::tempdir().unwrap();
        let config = dir.path().join("desktop.toml");
        std::fs::write(
            &config,
            "[mobile]\nprofile = \"env:MORTSOM_TEST_PROFILE_NEVER_SET\"\n",
        )
        .unwrap();

        assert_eq!(
            effective_profile(&config, None),
            mortsom_app_contract::config::DEFAULT_PROFILE
        );
        assert_eq!(effective_profile(&config, Some("cli")), "cli");
    }
}
