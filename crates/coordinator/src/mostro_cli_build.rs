//! Where the `cli` profile's mostro-cli comes from (spec §7): a prebuilt
//! executable, an explicit local checkout, or a branch or pull request of
//! `MostroP2P/mostro-cli` fetched and built here. Without a selection the
//! `main` branch is built, so ordinary runs exercise the CLI as it ships.

use std::path::PathBuf;

use crate::git_source::{self, Upstream};

/// The only repository accepted by remote CLI selectors.
pub const REPOSITORY: &str = "https://github.com/MostroP2P/mostro-cli.git";
/// Ignored workspace directory for the CLI's object cache and checkouts.
pub const WORK_DIR: &str = ".mortsom/mostro-cli";

/// The mostro-cli upstream.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MostroCli;

impl Upstream for MostroCli {
    const COMPONENT: &'static str = "mostro-cli";
    const REPOSITORY: &'static str = REPOSITORY;
    const PULL_PREFIX: &'static str = "https://github.com/MostroP2P/mostro-cli/pull/";
    const WORK_DIR: &'static str = WORK_DIR;
}

/// Validated mostro-cli branch name or GitHub pull-request URL.
pub type Selector = git_source::Selector<MostroCli>;

/// Which mostro-cli a `cli` run drives: `--artifact`, `--source` or `--ref`
/// read for profile `cli` (see [`crate::cli::ClientBuild`]).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CliSelection {
    /// Prebuilt mostro-cli executable.
    pub cli_artifact: Option<PathBuf>,
    /// mostro-cli branch or https://github.com/MostroP2P/mostro-cli/pull/123
    /// (its head commit) to build; `main` when nothing is selected.
    pub cli_ref: Option<Selector>,
    /// Local mostro-cli checkout to build, including uncommitted changes.
    pub cli_source: Option<PathBuf>,
}

impl CliSelection {
    /// Whether anything about the CLI was selected.
    pub fn is_empty(&self) -> bool {
        self.cli_artifact.is_none() && self.cli_ref.is_none() && self.cli_source.is_none()
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn a_pull_request_of_mostro_cli_is_built_from_its_head() {
        let selector = Selector::parse("https://github.com/MostroP2P/mostro-cli/pull/233").unwrap();

        assert_eq!(selector.git_ref(), "refs/pull/233/head");
        assert_eq!(Selector::main().git_ref(), "refs/heads/main");
    }

    #[test]
    fn other_repositories_and_odd_selectors_are_refused() {
        for invalid in [
            "https://github.com/MostroP2P/mostro/pull/963",
            "https://github.com/MostroP2P/app/pull/1",
            "https://github.com/someone/mostro-cli/pull/2",
            "https://github.com/MostroP2P/mostro-cli/pull/2/files",
            "--upload-pack=evil",
            "",
        ] {
            assert!(Selector::parse(invalid).is_err(), "{invalid}");
        }
    }

    #[test]
    fn nothing_selected_is_an_empty_selection() {
        assert!(CliSelection::default().is_empty());
        let selected = CliSelection {
            cli_ref: Some(Selector::main()),
            ..CliSelection::default()
        };
        assert!(!selected.is_empty());
    }
}
