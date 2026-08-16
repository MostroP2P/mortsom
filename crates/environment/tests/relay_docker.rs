//! Real Docker integration: starts a nostr-rs-relay container, proves the
//! Nostr readiness probe and tears it down. Ignored by default; run with
//! `cargo test -p mortsom-environment --test relay_docker -- --ignored`.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::time::Duration;

use mortsom_environment::config::RelayConfig;
use mortsom_environment::docker::{ContainerRuntime, DockerCli};
use mortsom_environment::relay::{NostrProbe, start_relay, teardown_relay};

#[tokio::test]
#[ignore = "requires a working Docker engine and network access to pull the relay image"]
async fn relay_starts_answers_nostr_and_tears_down() {
    let dir = tempfile::tempdir().unwrap();
    let docker = DockerCli::default();
    let config = RelayConfig {
        host_port: 47100,
        ..RelayConfig::default()
    };
    let run_id = format!("it{}", std::process::id());

    let handle = start_relay(
        &docker,
        &NostrProbe,
        &config,
        &run_id,
        dir.path(),
        Duration::from_secs(60),
    )
    .await
    .expect("relay should become ready");
    // Observed before the teardown and asserted after it: a failed
    // expectation must not leave a container running on the host.
    let running = docker.is_running(&handle.container_name).await;
    let listed = docker.list_names("mortsom-relay-it").await;

    let (teardown, logs) = teardown_relay(&docker, &handle, false).await;

    assert!(running.unwrap(), "the relay should have been running");
    assert!(
        listed.unwrap().contains(&handle.container_name),
        "the relay should have been listed"
    );
    assert!(teardown.removed, "{teardown:?}");
    assert!(teardown.data_removed);
    assert!(!docker.is_running(&handle.container_name).await.unwrap());
    assert!(!logs.is_empty());
}
