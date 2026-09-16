//! Builds a fetched Mostro checkout into a local image named after its
//! exact commit.
use std::collections::BTreeMap;
use std::time::Duration;

use mortsom_ui_driver::process::{Command, CommandRunner, run_ok};
use serde::Deserialize;

use super::{ExpectedImage, INSPECT_TIMEOUT, invalid, valid_digest};
use crate::git_source::PreparedSource;
use crate::stack::StackError;

/// Local repository every source build is tagged into; never pushed.
pub const LOCAL_REPOSITORY: &str = "mortsom/mostro";
/// The repository's supported build path, relative to its root.
const DOCKERFILE: &str = "docker/Dockerfile";
/// Where the build records the commit it was made from.
const REVISION_LABEL: &str = "org.opencontainers.image.revision";
const SOURCE_LABEL: &str = "org.opencontainers.image.source";
/// A first build compiles mostrod from scratch.
const BUILD_TIMEOUT: Duration = Duration::from_secs(3600);
const LABELS_FORMAT: &str = r#"{"id":{{json .Id}},"labels":{{json .Config.Labels}}}"#;

#[derive(Deserialize)]
struct BuiltMetadata {
    id: String,
    labels: Option<BTreeMap<String, String>>,
}

/// The tag a commit builds into. The full commit keeps it unambiguous.
pub fn commit_tag(revision: &str) -> String {
    format!("{LOCAL_REPOSITORY}:git-{revision}")
}

/// Builds `checkout` and returns the image the build produced. Every call
/// builds: BuildKit's layer cache makes a repeated commit cheap, and a
/// failed build never leaves an older image standing in for it.
pub(in crate::stack) async fn build(
    runner: &dyn CommandRunner,
    checkout: &PreparedSource,
) -> Result<ExpectedImage, StackError> {
    let revision = checkout.selection.revision.as_str();
    if revision.len() != 40 || !revision.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(invalid(
            "the fetched Mostro revision is not a full commit ID",
        ));
    }
    let dockerfile = checkout.path.join(DOCKERFILE);
    if !dockerfile.is_file() {
        return Err(StackError::Request(format!(
            "Mostro {} ({revision}) has no {DOCKERFILE}; it cannot be built here",
            checkout.selection.requested
        )));
    }
    // `COPY . .` would hash the checkout's Git metadata, which differs on
    // every clone and would defeat the layer cache for the same commit.
    remove_git_metadata(&checkout.path)?;
    let tag = commit_tag(revision);
    let short = format!("{LOCAL_REPOSITORY}:{}", &revision[..12]);
    let revision_label = format!("{REVISION_LABEL}={revision}");
    let source_label = format!("{SOURCE_LABEL}={}", checkout.selection.repository);
    let context = checkout.path.display().to_string();
    let dockerfile = dockerfile.display().to_string();
    tracing::info!(
        image = tag,
        selector = checkout.selection.requested,
        "building mostrod; the first build of a commit compiles it and takes several minutes"
    );
    run_ok(
        runner,
        &Command::new(
            "docker",
            &[
                "build",
                "--file",
                &dockerfile,
                "--tag",
                &tag,
                "--tag",
                &short,
                "--label",
                &revision_label,
                "--label",
                &source_label,
                &context,
            ],
        ),
        BUILD_TIMEOUT,
    )
    .await?;
    inspect_built(runner, &tag, revision).await
}

/// The image `tag` names, provided it carries `revision`.
async fn inspect_built(
    runner: &dyn CommandRunner,
    tag: &str,
    revision: &str,
) -> Result<ExpectedImage, StackError> {
    let output = run_ok(
        runner,
        &Command::new(
            "docker",
            &["image", "inspect", "--format", LABELS_FORMAT, tag],
        ),
        INSPECT_TIMEOUT,
    )
    .await?;
    let metadata: BuiltMetadata = serde_json::from_str(&output.stdout)
        .map_err(|_| invalid("invalid Docker image metadata"))?;
    if !valid_digest(&metadata.id) {
        return Err(invalid("invalid Docker image ID"));
    }
    let labelled = metadata
        .labels
        .as_ref()
        .and_then(|labels| labels.get(REVISION_LABEL));
    if labelled.map(String::as_str) != Some(revision) {
        return Err(invalid(
            "the built Mostro image does not carry the fetched commit",
        ));
    }
    Ok(ExpectedImage {
        id: metadata.id,
        reference: tag.to_owned(),
    })
}

fn remove_git_metadata(root: &std::path::Path) -> Result<(), StackError> {
    let git = root.join(".git");
    std::fs::remove_dir_all(&git).map_err(|error| {
        StackError::Configuration(format!("cannot remove {}: {error}", git.display()))
    })
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::git_source::RemoteSelection;
    use mortsom_ui_driver::process::tests_support::FakeRunner;

    const REVISION: &str = "0123456789abcdef0123456789abcdef01234567";

    fn checkout(with_dockerfile: bool) -> (tempfile::TempDir, PreparedSource) {
        let root = tempfile::tempdir().unwrap();
        let directory = tempfile::tempdir_in(root.path()).unwrap();
        let path = directory.path().join("source");
        std::fs::create_dir_all(path.join(".git")).unwrap();
        if with_dockerfile {
            std::fs::create_dir_all(path.join("docker")).unwrap();
            std::fs::write(path.join(DOCKERFILE), "FROM scratch\n").unwrap();
        }
        let prepared = PreparedSource::new(
            path,
            RemoteSelection {
                repository: crate::mostro_source::REPOSITORY.into(),
                requested: "main".into(),
                git_ref: "refs/heads/main".into(),
                revision: REVISION.into(),
            },
            directory,
        );
        (root, prepared)
    }

    fn inspected(labels: serde_json::Value) -> String {
        serde_json::json!({"id": format!("sha256:{}", "a".repeat(64)), "labels": labels})
            .to_string()
    }

    #[tokio::test]
    async fn builds_the_checkout_under_its_commit_tags_with_the_commit_label() {
        // Arrange
        let (_root, checkout) = checkout(true);
        let runner = FakeRunner::default();
        runner.script(
            "docker image inspect",
            &inspected(serde_json::json!({REVISION_LABEL: REVISION})),
        );

        // Act
        let image = build(&runner, &checkout).await.unwrap();

        // Assert
        assert_eq!(image.reference, format!("mortsom/mostro:git-{REVISION}"));
        assert_eq!(image.id, format!("sha256:{}", "a".repeat(64)));
        let calls = runner.calls();
        let build_call = calls
            .iter()
            .find(|c| c.starts_with("docker build"))
            .unwrap();
        for expected in [
            format!("--tag mortsom/mostro:git-{REVISION}"),
            "--tag mortsom/mostro:0123456789ab".to_owned(),
            format!("--label {REVISION_LABEL}={REVISION}"),
            format!("--file {}", checkout.path.join(DOCKERFILE).display()),
        ] {
            assert!(build_call.contains(&expected), "{build_call}");
        }
        assert!(build_call.ends_with(&checkout.path.display().to_string()));
        assert!(!checkout.path.join(".git").exists());
    }

    #[tokio::test]
    async fn an_image_that_does_not_carry_the_fetched_commit_is_refused() {
        for labels in [
            serde_json::Value::Null,
            serde_json::json!({}),
            serde_json::json!({REVISION_LABEL: "f".repeat(40)}),
        ] {
            // Arrange
            let (_root, checkout) = checkout(true);
            let runner = FakeRunner::default();
            runner.script("docker image inspect", &inspected(labels.clone()));

            // Act
            let verdict = build(&runner, &checkout).await;

            // Assert
            assert!(verdict.is_err(), "{labels}");
        }
    }

    #[tokio::test]
    async fn a_failed_build_is_not_followed_by_an_inspection() {
        // Arrange
        let (_root, checkout) = checkout(true);
        let runner = FakeRunner::default();
        runner.script_failure("docker build", "compile error");

        // Act
        let error = build(&runner, &checkout).await.unwrap_err();

        // Assert
        assert!(error.to_string().contains("compile error"), "{error}");
        assert!(
            runner
                .calls()
                .iter()
                .all(|call| !call.starts_with("docker image inspect"))
        );
    }

    #[tokio::test]
    async fn a_checkout_without_the_supported_dockerfile_is_not_built() {
        // Arrange
        let (_root, checkout) = checkout(false);
        let runner = FakeRunner::default();

        // Act
        let error = build(&runner, &checkout).await.unwrap_err();

        // Assert
        assert!(matches!(error, StackError::Request(_)), "{error}");
        assert!(runner.calls().is_empty());
    }
}
