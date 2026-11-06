//! Where the `cli` profile's mostro-cli comes from (spec §7): a prebuilt
//! executable, an explicit local checkout, or a branch or pull request of
//! `MostroP2P/mostro-cli` fetched and built here. Without a selection the
//! `main` branch is built, so ordinary runs exercise the CLI as it ships.

use std::path::{Path, PathBuf};
use std::time::Duration;

use mortsom_ui_driver::process::{Command, CommandRunner, TokioRunner, run_ok};

use crate::Config;
use crate::doctor::{HostProbe, RealHost};
use crate::git_source::{self, Upstream};
use crate::provenance::BuildProvenance;
use crate::run::DesktopOptions;

/// The only repository accepted by remote CLI selectors.
pub const REPOSITORY: &str = "https://github.com/MostroP2P/mostro-cli.git";
/// Ignored workspace directory for the CLI's object cache and checkouts.
pub const WORK_DIR: &str = ".mortsom/mostro-cli";
/// Cargo's target directory, shared by every CLI build so a repeated or
/// nearby commit compiles incrementally.
pub const TARGET_DIR: &str = ".mortsom/mostro-cli/target";
/// How long one release build may take.
const BUILD_TIMEOUT: Duration = Duration::from_secs(45 * 60);

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

/// Prepares the run's CLI build from the selection.
pub async fn prepare(config: &Config, selection: &CliSelection) -> Result<DesktopOptions, String> {
    if let Some(executable) = &selection.cli_artifact {
        let executable = executable
            .canonicalize()
            .map_err(|e| format!("mostro-cli executable {}: {e}", executable.display()))?;
        return crate::mostro_cli::prepare(config, &executable, BuildProvenance::prebuilt()).await;
    }
    let target = std::env::current_dir()
        .map_err(|e| e.to_string())?
        .join(TARGET_DIR);
    if let Some(source) = &selection.cli_source {
        let source = source
            .canonicalize()
            .map_err(|e| format!("mostro-cli checkout {}: {e}", source.display()))?;
        let (executable, provenance) = build(&source, &target, &TokioRunner, &RealHost).await?;
        return crate::mostro_cli::prepare(config, &executable, provenance).await;
    }
    let selector = selection.cli_ref.clone().unwrap_or_else(Selector::main);
    let fetched = git_source::prepare(&selector, &TokioRunner)
        .await
        .map_err(|e| e.to_string())?;
    let (executable, mut provenance) =
        build(&fetched.path, &target, &TokioRunner, &RealHost).await?;
    if provenance
        .source_before
        .as_ref()
        .is_none_or(|identity| identity.revision != fetched.selection.revision || identity.dirty)
    {
        return Err("managed mostro-cli source changed before the build".into());
    }
    provenance.remote_selection = Some(fetched.selection.clone());
    crate::mostro_cli::prepare(config, &executable, provenance).await
}

/// The release build of the `mostro-cli` binary in `source`.
pub fn build_command(source: &Path, target: &Path) -> Command {
    let chdir = format!("--chdir={}", source.display());
    let mut args = vec![chdir.as_str()];
    args.extend(RECORDED);
    Command::new("env", &args).env("CARGO_TARGET_DIR", &target.display().to_string())
}

/// The build command as provenance records it: never the checkout path.
const RECORDED: [&str; 6] = [
    "cargo",
    "build",
    "--release",
    "--locked",
    "--bin",
    "mostro-cli",
];

/// Builds `source` and returns the executable with its provenance.
async fn build(
    source: &Path,
    target: &Path,
    runner: &dyn CommandRunner,
    host: &dyn HostProbe,
) -> Result<(PathBuf, BuildProvenance), String> {
    let mut provenance = BuildProvenance::begin_cli(source, host).await?;
    tracing::info!("building mostro-cli");
    let started = std::time::Instant::now();
    let outcome = run_ok(runner, &build_command(source, target), BUILD_TIMEOUT).await;
    let elapsed = started.elapsed();
    match &outcome {
        Ok(_) => tracing::info!(?elapsed, "mostro-cli build finished"),
        Err(_) => tracing::warn!(?elapsed, "mostro-cli build failed"),
    }
    outcome.map_err(|e| mortsom_common::Redactor::default().redact(&e.to_string()))?;
    provenance.commands = vec![RECORDED.iter().map(|s| (*s).to_owned()).collect()];
    provenance.finish_cli(source, host).await?;
    Ok((target.join("release").join("mostro-cli"), provenance))
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

    #[test]
    fn the_build_is_locked_and_records_no_checkout_path() {
        let command = build_command(Path::new("/src/cli"), Path::new("/t"));

        assert_eq!(command.args[0], "--chdir=/src/cli");
        assert_eq!(&command.args[1..], RECORDED);
        assert_eq!(command.env["CARGO_TARGET_DIR"], "/t");
    }
}
