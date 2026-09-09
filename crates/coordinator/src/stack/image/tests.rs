#![allow(clippy::unwrap_used)]
use super::*;
use mortsom_android_driver::process::tests_support::FakeRunner;
use std::path::Path;

fn expected() -> ExpectedImage {
    ExpectedImage {
        id: format!("sha256:{}", "a".repeat(64)),
        reference: format!("mostrop2p/mostro@sha256:{}", "b".repeat(64)),
    }
}

#[tokio::test]
async fn resolves_the_selected_tag_to_an_immutable_official_reference() {
    let runner = FakeRunner::default();
    let image = expected();
    runner.script(
        "docker image inspect",
        &serde_json::json!({
            "id": image.id, "digests": [image.reference],
        })
        .to_string(),
    );
    assert_eq!(
        resolve(&runner, "mostrop2p/mostro:v0.18.7").await.unwrap(),
        image
    );
    assert!(
        runner
            .calls()
            .iter()
            .any(|c| c.ends_with("mostrop2p/mostro:v0.18.7")),
        "{:?}",
        runner.calls()
    );
}

#[tokio::test]
async fn refuses_invalid_or_non_official_image_metadata() {
    for value in [
        "not json".into(),
        serde_json::json!({
            "id": expected().id,
            "digests": [expected().reference, format!("mostrop2p/mostro@sha256:{}", "c".repeat(64))],
        }).to_string(),
        serde_json::json!({
            "id": expected().id, "digests": [format!("other/mostro@sha256:{}", "b".repeat(64))],
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
    for (id, running, accepted) in [
        (expected().id, true, true),
        (format!("sha256:{}", "c".repeat(64)), true, false),
        (expected().id, false, false),
    ] {
        let runner = FakeRunner::default();
        runner.script("env", &"d".repeat(64));
        runner.script(
            "docker inspect",
            &serde_json::json!({"image":id,"running":running,"working_dir":"/ortsom/ci/regtest", "config_files":"/ortsom/ci/regtest/compose.yml"}).to_string(),
        );
        assert_eq!(
            verify_running(&runner, Path::new("/ortsom"), &expected())
                .await
                .is_ok(),
            accepted
        );
        assert!(runner.calls().iter().all(|call| !call.contains("down")));
    }
}
