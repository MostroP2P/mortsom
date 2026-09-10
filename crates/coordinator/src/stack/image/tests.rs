#![allow(clippy::unwrap_used)]
use super::*;
use mortsom_ui_driver::process::tests_support::FakeRunner;

fn expected() -> ExpectedImage {
    ExpectedImage {
        id: format!("sha256:{}", "a".repeat(64)),
        reference: format!("mostrop2p/mostro@sha256:{}", "b".repeat(64)),
    }
}

fn layout() -> (tempfile::TempDir, Layout) {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("compose.yml"), "").unwrap();
    let layout = Layout::new(dir.path()).unwrap();
    (dir, layout)
}

#[tokio::test]
async fn resolves_the_selected_tag_to_an_immutable_official_reference() {
    // Arrange
    let runner = FakeRunner::default();
    let image = expected();
    runner.script(
        "docker image inspect",
        &serde_json::json!({"id": image.id, "digests": [image.reference]}).to_string(),
    );

    // Act
    let resolved = resolve(&runner, "mostrop2p/mostro:v0.18.7").await.unwrap();

    // Assert
    assert_eq!(resolved, image);
    assert!(
        runner
            .calls()
            .iter()
            .any(|call| call.ends_with("mostrop2p/mostro:v0.18.7")),
        "{:?}",
        runner.calls()
    );
}

#[tokio::test]
async fn refuses_invalid_or_non_official_image_metadata() {
    for value in [
        "not json".to_owned(),
        serde_json::json!({
            "id": expected().id,
            "digests": [expected().reference, format!("mostrop2p/mostro@sha256:{}", "c".repeat(64))],
        })
        .to_string(),
        serde_json::json!({
            "id": expected().id,
            "digests": [format!("other/mostro@sha256:{}", "b".repeat(64))],
        })
        .to_string(),
    ] {
        let runner = FakeRunner::default();
        runner.script("docker image inspect", &value);
        assert!(resolve(&runner, "mostrop2p/mostro:v0.18.7").await.is_err());
    }
}

#[tokio::test]
async fn refuses_a_stale_or_stopped_daemon_even_when_the_tag_name_matches() {
    let (_dir, layout) = layout();
    let compose = Compose::new(&layout, None);
    for (id, running, accepted) in [
        (expected().id, true, true),
        (format!("sha256:{}", "c".repeat(64)), true, false),
        (expected().id, false, false),
    ] {
        // Arrange
        let runner = FakeRunner::default();
        runner.script("docker compose --profile daemon ps -q", &"d".repeat(64));
        runner.script(
            "docker inspect",
            &serde_json::json!({
                "image": id, "running": running,
                "working_dir": layout.root().display().to_string(),
                "config_files": layout.compose_file().display().to_string(),
            })
            .to_string(),
        );

        // Act
        let verdict = verify_running(&runner, &compose, &layout, &expected()).await;

        // Assert
        assert_eq!(verdict.is_ok(), accepted, "{id} running={running}");
        assert!(runner.calls().iter().all(|call| !call.contains(" down")));
    }
}

#[tokio::test]
async fn refuses_a_daemon_container_another_directory_created() {
    // Arrange
    let (_dir, layout) = layout();
    let compose = Compose::new(&layout, None);
    let runner = FakeRunner::default();
    runner.script("docker compose --profile daemon ps -q", &"d".repeat(64));
    runner.script(
        "docker inspect",
        &serde_json::json!({
            "image": expected().id, "running": true,
            "working_dir": "/elsewhere", "config_files": "/elsewhere/compose.yml",
        })
        .to_string(),
    );

    // Act / Assert
    assert!(
        verify_running(&runner, &compose, &layout, &expected())
            .await
            .is_err()
    );
}
