//! Managed Mostro daemon checkouts, for building a branch or pull request
//! into the stack. Same guarantees as the app source: an exact fetched
//! commit, a disposable clean checkout and no cached fallback.
use crate::git_source::{self, Upstream};

pub use crate::git_source::{PreparedSource, RemoteSelection, SourceError};

/// The only repository accepted by remote daemon selectors.
pub const REPOSITORY: &str = "https://github.com/MostroP2P/mostro.git";
/// Ignored workspace directory for the daemon's object cache and checkouts.
pub const WORK_DIR: &str = ".mortsom/mostro";

/// The Mostro daemon upstream.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Mostro;

impl Upstream for Mostro {
    const COMPONENT: &'static str = "mostro";
    const REPOSITORY: &'static str = REPOSITORY;
    const PULL_PREFIX: &'static str = "https://github.com/MostroP2P/mostro/pull/";
    const WORK_DIR: &'static str = WORK_DIR;
}

/// Validated Mostro branch name or GitHub pull-request URL.
pub type Selector = git_source::Selector<Mostro>;

/// Fetches the requested daemon head, including GitHub's head ref for fork PRs.
pub async fn prepare(
    selector: &Selector,
    runner: &dyn mortsom_ui_driver::process::CommandRunner,
) -> Result<PreparedSource, SourceError> {
    git_source::prepare(selector, runner).await
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn upstream_branches_and_canonical_pull_requests_select_their_head() {
        // Arrange / Act / Assert
        assert_eq!(
            Selector::parse("feature/my-change").unwrap().git_ref(),
            "refs/heads/feature/my-change"
        );
        for url in [
            "https://github.com/MostroP2P/mostro/pull/963",
            "https://github.com/MostroP2P/mostro/pull/963/",
        ] {
            let selector = Selector::parse(url).unwrap();
            assert_eq!(selector.git_ref(), "refs/pull/963/head");
            assert_eq!(selector.requested(), url);
        }
    }

    #[test]
    fn other_repositories_file_views_queries_and_fragments_are_refused() {
        for invalid in [
            "",
            "HEAD",
            "refs/heads/main",
            "--upload-pack=evil",
            "https://github.com/MostroP2P/app/pull/963",
            "https://github.com/someone/mostro/pull/963",
            "https://github.com/MostroP2P/mostro/pull/963/files",
            "https://github.com/MostroP2P/mostro/pull/963?diff=split",
            "https://github.com/MostroP2P/mostro/pull/963#discussion",
            "https://github.com/MostroP2P/mostro/pull/0",
            "https://github.com/MostroP2P/mostro/pull/",
            "https://github.com/MostroP2P/mostro/tree/main",
        ] {
            let error = Selector::parse(invalid).unwrap_err().to_string();
            assert!(
                error.contains("invalid mostro selector"),
                "{invalid}: {error}"
            );
        }
    }
}
