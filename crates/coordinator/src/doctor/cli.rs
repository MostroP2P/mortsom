//! `doctor --profile cli`: what a mostro-cli run needs, without building,
//! launching an actor, creating an order or paying. A prebuilt executable is
//! checked through its offline discovery; a source selection needs Git and
//! Cargo, and its discovery is deferred until `run` builds it.

use std::path::Path;

use mortsom_cli_driver::CliProcess;
use mortsom_common::check::{CheckStatus, DoctorReport};

use crate::mostro_cli::{discover, require, validate_executable};
use crate::mostro_cli_build::CliSelection;

use super::HostProbe;

/// Host tools the selection needs.
pub(crate) fn check_tools(
    selection: &CliSelection,
    host: &dyn HostProbe,
    report: &mut DoctorReport,
) {
    let tools: &[&str] = if selection.cli_artifact.is_some() {
        &["docker"]
    } else {
        &["docker", "git", "cargo"]
    };
    for tool in tools {
        match host.which(tool) {
            Some(path) => report.push(*tool, CheckStatus::Pass, path.display().to_string()),
            None => report.push(*tool, CheckStatus::Fail, format!("{tool} not found")),
        }
    }
}

/// The CLI itself: its executable and discovery, run through `process`,
/// or its source.
pub(crate) async fn check(
    selection: &CliSelection,
    process: &dyn CliProcess,
    report: &mut DoctorReport,
) {
    if let Some(executable) = &selection.cli_artifact {
        check_executable(executable, process, report).await;
        return;
    }
    if let Some(source) = &selection.cli_source {
        if source.join("Cargo.toml").is_file() {
            report.push(
                "mostro-cli-source",
                CheckStatus::Pass,
                format!("{} (built by run)", source.display()),
            );
        } else {
            report.push(
                "mostro-cli-source",
                CheckStatus::Fail,
                format!("{} has no Cargo.toml", source.display()),
            );
        }
    } else {
        let requested = selection
            .cli_ref
            .as_ref()
            .map_or("main", |selector| selector.requested());
        report.push(
            "mostro-cli-source",
            CheckStatus::Skip,
            format!("{requested} of MostroP2P/mostro-cli is fetched and built by run"),
        );
    }
    report.push(
        "mostro-cli-commands",
        CheckStatus::Skip,
        "deferred until run builds the CLI",
    );
}

async fn check_executable(executable: &Path, process: &dyn CliProcess, report: &mut DoctorReport) {
    if let Err(error) = validate_executable(executable) {
        report.push("mostro-cli", CheckStatus::Fail, error);
        return;
    }
    match discover(executable, process).await {
        Err(error) => report.push("mostro-cli", CheckStatus::Fail, error),
        Ok(discovery) => {
            report.push(
                "mostro-cli",
                CheckStatus::Pass,
                format!(
                    "version {}, {} JSON commands",
                    discovery.version,
                    discovery.commands.len()
                ),
            );
            match require(&discovery) {
                Ok(()) => report.push(
                    "mostro-cli-commands",
                    CheckStatus::Pass,
                    "every command the cli profile needs",
                ),
                Err(error) => report.push("mostro-cli-commands", CheckStatus::Fail, error),
            }
        }
    }
}

/// A mostro-cli stand-in for tests: no child process is started.
#[cfg(test)]
#[allow(clippy::expect_used)]
pub(super) mod fake {
    use std::path::{Path, PathBuf};
    use std::sync::Mutex;
    use std::time::Duration;

    use async_trait::async_trait;
    use mortsom_cli_driver::{CliProcess, ProcessFailed, ProcessOutput, ProcessRequest};

    /// Answers every invocation with `stdout` and exit code 0, and records
    /// what it was asked to run.
    pub(crate) struct AnsweringCli {
        stdout: String,
        seen: Mutex<Vec<ProcessRequest>>,
    }

    impl AnsweringCli {
        pub(crate) fn new(stdout: impl Into<String>) -> Self {
            Self {
                stdout: stdout.into(),
                seen: Mutex::new(Vec::new()),
            }
        }

        /// The discovery of a CLI whose JSON commands are `commands`, its
        /// `order.add-invoice` reading the invoice from standard input.
        pub(crate) fn discovering(commands: &[&str]) -> Self {
            let commands: Vec<serde_json::Value> = commands
                .iter()
                .map(|name| {
                    let input = (*name == "order.add-invoice")
                        .then(|| serde_json::json!({"properties": {"invoice": {"x-stdin": "-"}}}));
                    serde_json::json!({
                        "name": name, "output_modes": ["json"], "input_schema": input
                    })
                })
                .collect();
            Self::new(
                serde_json::json!({
                    "schema_version": "1",
                    "command": "schema",
                    "status": "ok",
                    "data": {
                        "schema_version": "1", "executable_version": "0.17.0",
                        "commands": commands, "global_options": [],
                    },
                    "error": null,
                    "warnings": [],
                    "meta": {},
                })
                .to_string(),
            )
        }

        /// Every invocation, in order.
        pub(crate) fn seen(&self) -> Vec<ProcessRequest> {
            self.seen.lock().expect("the request log").clone()
        }

        /// How many times the CLI was invoked.
        pub(crate) fn calls(&self) -> usize {
            self.seen().len()
        }
    }

    #[async_trait]
    impl CliProcess for AnsweringCli {
        async fn run(
            &self,
            request: &ProcessRequest,
            _timeout: Duration,
        ) -> Result<ProcessOutput, ProcessFailed> {
            self.seen
                .lock()
                .expect("the request log")
                .push(request.clone());
            Ok(ProcessOutput {
                code: Some(0),
                stdout: self.stdout.clone(),
                stderr: String::new(),
            })
        }
    }

    /// A regular executable file named `mostro-cli` in `dir`; the fake
    /// never runs it.
    pub(crate) fn executable(dir: &Path) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let path = dir.join("mostro-cli");
        std::fs::write(&path, "").expect("write the stand-in executable");
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))
            .expect("make the stand-in executable");
        path
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::fake::{AnsweringCli, executable};
    use super::*;
    use crate::mostro_cli::REQUIRED_COMMANDS;
    use crate::mostro_cli_build::Selector;

    fn check_of<'a>(
        report: &'a DoctorReport,
        name: &str,
    ) -> &'a mortsom_common::check::CheckResult {
        report
            .checks
            .iter()
            .find(|check| check.name == name)
            .unwrap_or_else(|| panic!("missing check {name}: {:#?}", report.checks))
    }

    fn status_of(report: &DoctorReport, name: &str) -> CheckStatus {
        check_of(report, name).status
    }

    fn prebuilt(path: std::path::PathBuf) -> CliSelection {
        CliSelection {
            cli_artifact: Some(path),
            ..CliSelection::default()
        }
    }

    #[tokio::test]
    async fn an_executable_that_is_not_a_cli_fails_its_check() {
        let dir = tempfile::tempdir().unwrap();
        let cli = AnsweringCli::new("not json\n");
        let mut report = DoctorReport::default();

        check(&prebuilt(executable(dir.path())), &cli, &mut report).await;

        assert_eq!(status_of(&report, "mostro-cli"), CheckStatus::Fail);
        assert_eq!(cli.calls(), 1);
    }

    #[tokio::test]
    async fn a_file_that_is_not_executable_is_refused_before_it_runs() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("mostro-cli");
        std::fs::write(&path, "").unwrap();
        let cli = AnsweringCli::discovering(REQUIRED_COMMANDS);
        let mut report = DoctorReport::default();

        check(&prebuilt(path), &cli, &mut report).await;

        assert_eq!(status_of(&report, "mostro-cli"), CheckStatus::Fail);
        assert_eq!(cli.calls(), 0);
    }

    #[tokio::test]
    async fn a_cli_whose_discovery_passes_passes_its_checks() {
        let dir = tempfile::tempdir().unwrap();
        let cli = AnsweringCli::discovering(REQUIRED_COMMANDS);
        let mut report = DoctorReport::default();

        check(&prebuilt(executable(dir.path())), &cli, &mut report).await;

        let found = check_of(&report, "mostro-cli");
        assert_eq!(found.status, CheckStatus::Pass);
        assert_eq!(
            found.detail,
            format!("version 0.17.0, {} JSON commands", REQUIRED_COMMANDS.len())
        );
        assert_eq!(status_of(&report, "mostro-cli-commands"), CheckStatus::Pass);
    }

    /// The discovery is offline and private: `schema` alone, nothing on
    /// standard input, an empty environment, and a working directory of its
    /// own that is gone once the check ends.
    #[tokio::test]
    async fn the_discovery_runs_schema_alone_in_an_empty_environment() {
        let dir = tempfile::tempdir().unwrap();
        let path = executable(dir.path());
        let cli = AnsweringCli::discovering(REQUIRED_COMMANDS);
        let mut report = DoctorReport::default();

        check(&prebuilt(path.clone()), &cli, &mut report).await;

        let seen = cli.seen();
        assert_eq!(seen.len(), 1);
        let request = &seen[0];
        assert_eq!(request.program, path.canonicalize().unwrap());
        assert_eq!(request.args, ["--output", "json", "schema"]);
        assert!(request.env.is_empty(), "{:?}", request.env);
        assert!(request.stdin.is_none());
        assert_ne!(request.current_dir, std::env::current_dir().unwrap());
        assert_ne!(request.current_dir, dir.path());
        assert!(!request.current_dir.exists());
    }

    #[tokio::test]
    async fn a_cli_without_a_required_command_fails_the_command_check() {
        let dir = tempfile::tempdir().unwrap();
        let commands: Vec<&str> = REQUIRED_COMMANDS
            .iter()
            .copied()
            .filter(|command| *command != "order.release")
            .collect();
        let cli = AnsweringCli::discovering(&commands);
        let mut report = DoctorReport::default();

        check(&prebuilt(executable(dir.path())), &cli, &mut report).await;

        assert_eq!(status_of(&report, "mostro-cli"), CheckStatus::Pass);
        let commands = check_of(&report, "mostro-cli-commands");
        assert_eq!(commands.status, CheckStatus::Fail);
        assert!(
            commands.detail.contains("order.release"),
            "{}",
            commands.detail
        );
    }

    #[tokio::test]
    async fn no_selection_defers_main_to_the_build() {
        let cli = AnsweringCli::new("");
        let mut report = DoctorReport::default();

        check(&CliSelection::default(), &cli, &mut report).await;

        let source = check_of(&report, "mostro-cli-source");
        assert_eq!(source.status, CheckStatus::Skip);
        assert!(
            source.detail.starts_with("main of MostroP2P/mostro-cli"),
            "{}",
            source.detail
        );
        assert_eq!(status_of(&report, "mostro-cli-commands"), CheckStatus::Skip);
        assert_eq!(cli.calls(), 0);
    }

    #[tokio::test]
    async fn a_ref_selection_names_the_requested_head() {
        let selection = CliSelection {
            cli_ref: Some(
                Selector::parse("https://github.com/MostroP2P/mostro-cli/pull/233").unwrap(),
            ),
            ..CliSelection::default()
        };
        let mut report = DoctorReport::default();

        check(&selection, &AnsweringCli::new(""), &mut report).await;

        let source = check_of(&report, "mostro-cli-source");
        assert_eq!(source.status, CheckStatus::Skip);
        assert!(source.detail.contains("pull/233"), "{}", source.detail);
        assert_eq!(status_of(&report, "mostro-cli-commands"), CheckStatus::Skip);
    }

    #[tokio::test]
    async fn a_local_source_with_a_manifest_defers_its_discovery() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("Cargo.toml"), "[package]\n").unwrap();
        let selection = CliSelection {
            cli_source: Some(dir.path().to_path_buf()),
            ..CliSelection::default()
        };
        let mut report = DoctorReport::default();

        check(&selection, &AnsweringCli::new(""), &mut report).await;

        assert_eq!(status_of(&report, "mostro-cli-source"), CheckStatus::Pass);
        assert_eq!(status_of(&report, "mostro-cli-commands"), CheckStatus::Skip);
    }

    #[tokio::test]
    async fn a_local_source_without_a_manifest_says_what_is_missing() {
        let dir = tempfile::tempdir().unwrap();
        let selection = CliSelection {
            cli_source: Some(dir.path().to_path_buf()),
            ..CliSelection::default()
        };
        let mut report = DoctorReport::default();

        check(&selection, &AnsweringCli::new(""), &mut report).await;

        let source = check_of(&report, "mostro-cli-source");
        assert_eq!(source.status, CheckStatus::Fail);
        assert_eq!(
            source.detail,
            format!("{} has no Cargo.toml", dir.path().display())
        );
    }
}
