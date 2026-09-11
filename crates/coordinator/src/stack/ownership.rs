//! Checks that the Compose project belongs to this stack directory before
//! any mutation, so `up` and `down` never touch containers another
//! directory created -- unless `up --replace` releases the project by name.

use super::StackError;
use super::layout::{Layout, PROJECT};
use mortsom_ui_driver::process::{Command, CommandRunner, run_ok};
use std::{path::Path, time::Duration};

const LABEL_FORMAT: &str = r#"{"working_dir":{{json (index .Config.Labels "com.docker.compose.project.working_dir")}},"config_files":{{json (index .Config.Labels "com.docker.compose.project.config_files")}}}"#;
const INSPECT_TIMEOUT: Duration = Duration::from_secs(30);
/// As long as `down` gets: the nodes and the daemon take a while to stop.
const RELEASE_TIMEOUT: Duration = Duration::from_secs(600);

#[derive(serde::Deserialize)]
struct Ownership {
    working_dir: String,
    config_files: String,
}

pub(super) async fn verify(runner: &dyn CommandRunner, layout: &Layout) -> Result<(), StackError> {
    let output = run_ok(
        runner,
        &Command::new(
            "docker",
            &[
                "ps",
                "--all",
                "--no-trunc",
                "--filter",
                &project_label(),
                "--format",
                "{{.ID}}",
            ],
        ),
        INSPECT_TIMEOUT,
    )
    .await?;
    if output.stdout.trim().is_empty() {
        return verify_empty_project(runner).await;
    }
    for id in output.stdout.lines() {
        verify_container(runner, layout, id).await?;
    }
    Ok(())
}

async fn verify_empty_project(runner: &dyn CommandRunner) -> Result<(), StackError> {
    for resource in ["volume", "network"] {
        if !labelled(runner, resource).await?.is_empty() {
            return Err(StackError::Unowned(format!(
                "{resource}s remain without containers to show which directory created them"
            )));
        }
    }
    Ok(())
}

pub(super) async fn verify_container(
    runner: &dyn CommandRunner,
    layout: &Layout,
    id: &str,
) -> Result<(), StackError> {
    if id.len() != 64 || !id.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(unreadable());
    }
    let output = run_ok(
        runner,
        &Command::new("docker", &["inspect", "--format", LABEL_FORMAT, id]),
        INSPECT_TIMEOUT,
    )
    .await?;
    let labels: Ownership = serde_json::from_str(&output.stdout).map_err(|_| unreadable())?;
    if Path::new(&labels.working_dir) != layout.root() {
        return Err(StackError::ForeignOwner(labels.working_dir));
    }
    if Path::new(&labels.config_files) != layout.compose_file() {
        return Err(StackError::Unowned(format!(
            "its containers were started from {}, not {}",
            labels.config_files,
            layout.compose_file().display()
        )));
    }
    Ok(())
}

/// Removes the project by name, whichever directory created it: containers
/// and volumes through Compose, which needs no compose file for that, then
/// whatever Compose leaves when no container remained to find it by.
pub(super) async fn release(runner: &dyn CommandRunner) -> Result<(), StackError> {
    run_ok(
        runner,
        &Command::new(
            "docker",
            &["compose", "-p", PROJECT, "down", "-v", "--remove-orphans"],
        ),
        RELEASE_TIMEOUT,
    )
    .await?;
    for resource in ["volume", "network"] {
        for name in labelled(runner, resource).await? {
            run_ok(
                runner,
                &Command::new("docker", &[resource, "rm", &name]),
                INSPECT_TIMEOUT,
            )
            .await?;
        }
    }
    Ok(())
}

/// Names of the project's volumes or networks.
async fn labelled(runner: &dyn CommandRunner, resource: &str) -> Result<Vec<String>, StackError> {
    let output = run_ok(
        runner,
        &Command::new(
            "docker",
            &[
                resource,
                "ls",
                "--filter",
                &project_label(),
                "--format",
                "{{.Name}}",
            ],
        ),
        INSPECT_TIMEOUT,
    )
    .await?;
    Ok(output
        .stdout
        .lines()
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(str::to_owned)
        .collect())
}

fn project_label() -> String {
    format!("label=com.docker.compose.project={PROJECT}")
}

fn unreadable() -> StackError {
    StackError::Unowned("its containers carry unreadable ownership labels".into())
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use mortsom_ui_driver::process::tests_support::FakeRunner;

    fn selected() -> (tempfile::TempDir, super::Layout) {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("compose.yml"), "").unwrap();
        let layout = super::Layout::new(dir.path()).unwrap();
        (dir, layout)
    }

    #[tokio::test]
    async fn resource_inspection_failure_refuses_adoption() {
        let (_dir, layout) = selected();
        let runner = FakeRunner::default();
        runner.script_failure("docker volume ls", "daemon unavailable");
        assert!(super::verify(&runner, &layout).await.is_err());
    }

    #[tokio::test]
    async fn leftover_resources_without_container_ownership_are_not_adopted() {
        let (_dir, layout) = selected();
        for command in ["docker volume ls", "docker network ls"] {
            let runner = FakeRunner::default();
            runner.script(command, "leftover-project-resource");
            assert!(super::verify(&runner, &layout).await.is_err());
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
        let (_dir, layout) = selected();
        let runner = FakeRunner::default();
        runner.script("docker ps", &"a".repeat(64));
        runner.script(
            "docker inspect",
            r#"{"working_dir":"/foreign","config_files":"/foreign/compose.yml"}"#,
        );
        assert!(super::verify(&runner, &layout).await.is_err());
        assert!(runner.calls().iter().all(|call| !call.contains("stack up")));
    }
}
