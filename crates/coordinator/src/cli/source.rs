//! The build of the client under test: `--artifact`, `--source` or `--ref`
//! select the app (profile `mobile-v2`).
use clap::Args;
use std::path::PathBuf;

/// Which build of the profile's client a run or a check uses. With none,
/// the profile's default: the app's configured source.
#[derive(Debug, Clone, Default, Args)]
pub struct ClientBuild {
    /// Prebuilt client: the app's native executable or Web bundle
    /// (`mobile-v2`).
    #[arg(long, value_name = "PATH", conflicts_with_all = ["source", "git_ref"])]
    pub artifact: Option<PathBuf>,
    /// Local checkout of the profile's client, built with its uncommitted
    /// changes.
    #[arg(long, value_name = "PATH", conflicts_with = "git_ref", value_parser = local_source)]
    pub source: Option<PathBuf>,
    /// Branch, or pull request URL, of the profile's repository
    /// (https://github.com/MostroP2P/app/pull/123); its head commit is
    /// built.
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
}

impl ClientBuild {
    /// Resolves the selection for the app: `--ref` must name a branch or a
    /// pull request of the app's repository.
    pub fn for_app(&self) -> Result<ClientSelection, String> {
        let invalid = |error: crate::git_source::SourceError| format!("--ref: {error}");
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
        })
    }
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
        .for_app()
        .unwrap();
        assert!(matches!(
            selected.source.selection(),
            Some(crate::config::AppSource::Remote(_))
        ));

        let selected = build(Some("/bundle"), None, None).for_app().unwrap();
        assert_eq!(selected.artifact, Some(PathBuf::from("/bundle")));
        assert!(selected.source.selection().is_none());
    }

    #[test]
    fn a_pull_request_of_the_other_repository_is_refused() {
        let cli_pull = build(
            None,
            None,
            Some("https://github.com/MostroP2P/mostro-cli/pull/2"),
        );
        let error = cli_pull.for_app().unwrap_err();
        assert!(error.starts_with("--ref:"), "{error}");
        assert!(
            build(None, None, Some("https://github.com/wrong/app/pull/2"))
                .for_app()
                .is_err()
        );
    }
}
