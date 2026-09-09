//! Checks the Compose checkout before any persistent stack mutation.

use super::StackError;
use mortsom_android_driver::process::{Command, CommandRunner, run_ok};
use std::{path::Path, time::Duration};

const LABEL_FORMAT: &str = r#"{"working_dir":{{json (index .Config.Labels "com.docker.compose.project.working_dir")}},"config_files":{{json (index .Config.Labels "com.docker.compose.project.config_files")}}}"#;

#[derive(serde::Deserialize)]
struct Ownership {
    working_dir: String,
    config_files: String,
}

pub(super) async fn verify(runner: &dyn CommandRunner, checkout: &Path) -> Result<(), StackError> {
    let output = run_ok(
        runner,
        &Command::new(
            "docker",
            &[
                "ps",
                "--all",
                "--no-trunc",
                "--filter",
                "label=com.docker.compose.project=ortsom-regtest",
                "--format",
                "{{.ID}}",
            ],
        ),
        Duration::from_secs(30),
    )
    .await?;
    if output.stdout.trim().is_empty() {
        return verify_empty_project(runner).await;
    }
    for id in output.stdout.lines() {
        verify_container(runner, checkout, id).await?;
    }
    Ok(())
}

async fn verify_empty_project(runner: &dyn CommandRunner) -> Result<(), StackError> {
    for resource in ["volume", "network"] {
        let output = run_ok(
            runner,
            &Command::new(
                "docker",
                &[
                    resource,
                    "ls",
                    "--filter",
                    "label=com.docker.compose.project=ortsom-regtest",
                    "--format",
                    "{{.Name}}",
                ],
            ),
            Duration::from_secs(30),
        )
        .await?;
        if !output.stdout.trim().is_empty() {
            return Err(StackError::Configuration(
                "project resources remain without containers to establish checkout ownership; refusing mutation".into(),
            ));
        }
    }
    Ok(())
}

pub(super) async fn verify_container(
    runner: &dyn CommandRunner,
    checkout: &Path,
    id: &str,
) -> Result<(), StackError> {
    if id.len() != 64 || !id.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(invalid());
    }
    let output = run_ok(
        runner,
        &Command::new("docker", &["inspect", "--format", LABEL_FORMAT, id]),
        Duration::from_secs(30),
    )
    .await?;
    let labels: Ownership = serde_json::from_str(&output.stdout).map_err(|_| invalid())?;
    let directory = checkout.join("ci/regtest");
    if Path::new(&labels.working_dir) != directory
        || Path::new(&labels.config_files) != directory.join("compose.yml")
    {
        return Err(invalid());
    }
    Ok(())
}

fn invalid() -> StackError {
    StackError::Configuration(
        "Compose project ownership does not match the selected Ortsom checkout".into(),
    )
}

#[cfg(test)]
mod tests {
    use mortsom_android_driver::process::tests_support::FakeRunner;

    #[tokio::test]
    async fn resource_inspection_failure_refuses_adoption() {
        let runner = FakeRunner::default();
        runner.script_failure("docker volume ls", "daemon unavailable");
        assert!(
            super::verify(&runner, std::path::Path::new("/selected"))
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn leftover_resources_without_container_ownership_are_not_adopted() {
        for command in ["docker volume ls", "docker network ls"] {
            let runner = FakeRunner::default();
            runner.script(command, "leftover-project-resource");
            assert!(
                super::verify(&runner, std::path::Path::new("/selected"))
                    .await
                    .is_err()
            );
            assert!(
                runner
                    .calls()
                    .iter()
                    .all(|call| !call.contains(" rm") && !call.contains("stack up"))
            );
        }
    }

    #[tokio::test]
    async fn foreign_checkout_is_rejected_before_stack_mutation() {
        let runner = FakeRunner::default();
        runner.script("docker ps", &"a".repeat(64));
        runner.script(
            "docker inspect",
            r#"{"working_dir":"/foreign","config_files":"/foreign/ci/regtest/compose.yml"}"#,
        );
        assert!(
            super::verify(&runner, std::path::Path::new("/selected"))
                .await
                .is_err()
        );
        assert!(runner.calls().iter().all(|call| !call.contains("stack up")));
    }
}
