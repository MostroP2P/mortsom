//! `doctor --profile cli`: what a mostro-cli run needs, without building,
//! launching an actor, creating an order or paying. A prebuilt executable is
//! checked through its offline discovery; a source selection needs Git and
//! Cargo, and its discovery is deferred until `run` builds it.

use std::path::Path;

use mortsom_cli_driver::TokioCliProcess;
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

/// The CLI itself: its executable and discovery, or its source.
pub(crate) async fn check(selection: &CliSelection, report: &mut DoctorReport) {
    if let Some(executable) = &selection.cli_artifact {
        check_executable(executable, report).await;
        return;
    }
    if let Some(source) = &selection.cli_source {
        let status = if source.join("Cargo.toml").is_file() {
            CheckStatus::Pass
        } else {
            CheckStatus::Fail
        };
        report.push(
            "mostro-cli-source",
            status,
            format!("{} (built by run)", source.display()),
        );
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

async fn check_executable(executable: &Path, report: &mut DoctorReport) {
    if let Err(error) = validate_executable(executable) {
        report.push("mostro-cli", CheckStatus::Fail, error);
        return;
    }
    match discover(executable, &TokioCliProcess).await {
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

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn status_of(report: &DoctorReport, name: &str) -> CheckStatus {
        report
            .checks
            .iter()
            .find(|check| check.name == name)
            .map(|check| check.status)
            .unwrap()
    }

    #[tokio::test]
    async fn an_executable_that_is_not_a_cli_fails_its_check() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("mostro-cli");
        std::fs::write(&path, "#!/bin/sh\necho not json\n").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        let selection = CliSelection {
            cli_artifact: Some(path),
            ..CliSelection::default()
        };
        let mut report = DoctorReport::default();

        check(&selection, &mut report).await;

        assert_eq!(status_of(&report, "mostro-cli"), CheckStatus::Fail);
    }

    #[tokio::test]
    async fn a_source_selection_defers_the_discovery_to_the_build() {
        let mut report = DoctorReport::default();

        check(&CliSelection::default(), &mut report).await;

        assert_eq!(status_of(&report, "mostro-cli-source"), CheckStatus::Skip);
        assert_eq!(status_of(&report, "mostro-cli-commands"), CheckStatus::Skip);
    }
}
