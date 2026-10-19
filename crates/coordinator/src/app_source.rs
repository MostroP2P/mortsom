//! Managed app checkouts: fetch a head once, then build an isolated exact revision.
use std::path::Path;
use std::time::Duration;

use mortsom_ui_driver::process::{Command, CommandRunner, run_ok};

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

/// What binding generation writes. Every app revision since 9e8ab29f
/// (2026-09-14) commits it, and app CI fails a pull request that drifts.
const GENERATED_BINDINGS: [&str; 2] = [
    "lib/src/rust/frb_generated.dart",
    "rust/src/frb_generated.rs",
];

/// Whether `checkout` commits the bindings generation would write. Any
/// failure, Git's own included, answers no.
pub(crate) async fn commits_bindings(checkout: &Path, runner: &dyn CommandRunner) -> bool {
    run_ok(runner, &bindings_check(checkout), Duration::from_secs(60))
        .await
        .is_ok()
}

/// The Git command [`commits_bindings`] runs.
pub(crate) fn bindings_check(checkout: &Path) -> Command {
    git_source::tracks_command(checkout, &GENERATED_BINDINGS)
}

/// Fetches the requested app head, including GitHub's head ref for fork PRs.
pub async fn prepare(
    selector: &Selector,
    runner: &dyn CommandRunner,
) -> Result<PreparedSource, SourceError> {
    git_source::prepare(selector, runner).await
}
