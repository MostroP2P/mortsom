//! Immutable identity checks for the daemon image selected at stack up.
use std::time::Duration;

use mortsom_ui_driver::process::{Command, CommandRunner, run_ok};
use serde::Deserialize;

use super::StackError;
use super::compose::{Compose, DAEMON_SERVICE};
use super::layout::Layout;

const INSPECT_TIMEOUT: Duration = Duration::from_secs(30);
const IMAGE_FORMAT: &str = r#"{"id":{{json .Id}},"digests":{{json .RepoDigests}}}"#;
const CONTAINER_FORMAT: &str = r#"{"image":{{json .Image}},"running":{{json .State.Running}}}"#;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ExpectedImage {
    pub id: String,
    pub reference: String,
}

#[derive(Deserialize)]
struct ImageMetadata {
    id: String,
    digests: Vec<String>,
}

#[derive(Deserialize)]
struct ContainerMetadata {
    image: String,
    running: bool,
}

/// Resolves the already pulled `image` tag to an immutable registry reference.
pub(super) async fn resolve(
    runner: &dyn CommandRunner,
    image: &str,
) -> Result<ExpectedImage, StackError> {
    let output = run_ok(
        runner,
        &Command::new(
            "docker",
            &["image", "inspect", "--format", IMAGE_FORMAT, image],
        ),
        INSPECT_TIMEOUT,
    )
    .await?;
    let metadata: ImageMetadata = serde_json::from_str(&output.stdout)
        .map_err(|_| invalid("invalid Docker image metadata"))?;
    if !valid_digest(&metadata.id) {
        return Err(invalid("invalid Docker image ID"));
    }
    let references: Vec<_> = metadata
        .digests
        .into_iter()
        .filter(|value| {
            value
                .strip_prefix("mostrop2p/mostro@")
                .is_some_and(valid_digest)
        })
        .collect();
    let [reference] = references.as_slice() else {
        return Err(invalid(
            "the selected image must have exactly one official immutable repository digest",
        ));
    };
    Ok(ExpectedImage {
        id: metadata.id,
        reference: reference.clone(),
    })
}

/// Checks the daemon container this project is actually running.
pub(super) async fn verify_running(
    runner: &dyn CommandRunner,
    compose: &Compose,
    layout: &Layout,
    expected: &ExpectedImage,
) -> Result<(), StackError> {
    let output = run_ok(
        runner,
        &compose.command(&["--profile", "daemon", "ps", "-q", DAEMON_SERVICE]),
        INSPECT_TIMEOUT,
    )
    .await?;
    let container = output.stdout.trim();
    if container.len() != 64 || !container.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(invalid(
            "expected exactly one Mostro container in the selected stack",
        ));
    }
    super::ownership::verify_container(runner, layout, container).await?;
    let output = run_ok(
        runner,
        &Command::new(
            "docker",
            &["inspect", "--format", CONTAINER_FORMAT, container],
        ),
        INSPECT_TIMEOUT,
    )
    .await?;
    let metadata: ContainerMetadata = serde_json::from_str(&output.stdout)
        .map_err(|_| invalid("invalid Docker container metadata"))?;
    if !metadata.running || metadata.image != expected.id {
        return Err(invalid("running mostrod does not match the selected image"));
    }
    Ok(())
}

fn valid_digest(value: &str) -> bool {
    value
        .strip_prefix("sha256:")
        .is_some_and(|hash| hash.len() == 64 && hash.chars().all(|c| c.is_ascii_hexdigit()))
}

fn invalid(message: &str) -> StackError {
    StackError::Configuration(message.into())
}

#[cfg(test)]
mod tests;
