//! Managed app checkouts: fetch a head once, then build an isolated exact revision.
use crate::git_source::{self, Upstream};

pub use crate::git_source::{PreparedSource, RemoteSelection, SourceError};

/// The only repository accepted by remote app selectors.
pub const REPOSITORY: &str = "https://github.com/MostroP2P/app.git";
/// Ignored workspace directory; source and build outputs never enter Mortsom Git.
pub const WORK_DIR: &str = ".mortsom/app";

/// The app v2 upstream.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct App;

impl Upstream for App {
    const COMPONENT: &'static str = "app";
    const REPOSITORY: &'static str = REPOSITORY;
    const PULL_PREFIX: &'static str = "https://github.com/MostroP2P/app/pull/";
    const WORK_DIR: &'static str = WORK_DIR;
}

/// Validated app branch name or GitHub pull-request URL.
pub type Selector = git_source::Selector<App>;

/// Fetches the requested app head, including GitHub's head ref for fork PRs.
pub async fn prepare(
    selector: &Selector,
    runner: &dyn mortsom_ui_driver::process::CommandRunner,
) -> Result<PreparedSource, SourceError> {
    git_source::prepare(selector, runner).await
}
