#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::Mutex;

use super::*;

/// Records every runtime call and can fail startup on demand.
#[derive(Default)]
struct FakeRuntime {
    running: Mutex<Vec<String>>,
    removed: Mutex<Vec<String>>,
    fail_run: bool,
    report_running: bool,
    logs: String,
}

impl FakeRuntime {
    fn healthy() -> Self {
        Self {
            report_running: true,
            ..Self::default()
        }
    }
}

#[async_trait]
impl ContainerRuntime for FakeRuntime {
    async fn run(&self, spec: &ContainerSpec) -> Result<String, DockerError> {
        if self.fail_run {
            return Err(DockerError::Command {
                command: "run".into(),
                stderr: "Unable to find image".into(),
            });
        }
        self.running.lock().unwrap().push(spec.name.clone());
        Ok("cid".into())
    }
    async fn is_running(&self, name: &str) -> Result<bool, DockerError> {
        Ok(self.report_running && self.running.lock().unwrap().iter().any(|n| n == name))
    }
    async fn stop_and_remove(&self, name: &str) -> Result<(), DockerError> {
        self.running.lock().unwrap().retain(|n| n != name);
        self.removed.lock().unwrap().push(name.to_owned());
        Ok(())
    }
    async fn restart(&self, _name: &str) -> Result<(), DockerError> {
        Ok(())
    }
    async fn logs(&self, _name: &str) -> Result<String, DockerError> {
        Ok(self.logs.clone())
    }
    async fn list_names(&self, prefix: &str) -> Result<Vec<String>, DockerError> {
        Ok(self
            .running
            .lock()
            .unwrap()
            .iter()
            .filter(|n| n.starts_with(prefix))
            .cloned()
            .collect())
    }
}

struct FakeProbe(bool);

#[async_trait]
impl ReadinessProbe for FakeProbe {
    async fn is_ready(&self, _url: &str) -> bool {
        self.0
    }
}

fn config() -> RelayConfig {
    RelayConfig {
        // Use a high base so tests never collide with a real relay on 7000.
        host_port: 47000,
        ..RelayConfig::default()
    }
}

#[tokio::test]
async fn start_creates_named_container_data_dir_and_urls() {
    let dir = tempfile::tempdir().unwrap();
    let runtime = FakeRuntime::healthy();

    let handle = start_relay(
        &runtime,
        &FakeProbe(true),
        &config(),
        "run1",
        dir.path(),
        Duration::from_secs(1),
    )
    .await
    .unwrap();

    assert_eq!(handle.container_name, "mortsom-relay-run1");
    assert!(handle.data_dir.is_dir());
    assert!(handle.host_url().starts_with("ws://127.0.0.1:"));
    assert!(
        handle
            .docker_url()
            .starts_with("ws://host.docker.internal:")
    );
    assert!(handle.emulator_url().starts_with("ws://10.0.2.2:"));
    assert!(runtime.is_running("mortsom-relay-run1").await.unwrap());
}

#[tokio::test]
async fn failed_readiness_removes_only_this_runs_container_and_keeps_logs() {
    let dir = tempfile::tempdir().unwrap();
    let runtime = FakeRuntime {
        logs: "relay panicked".into(),
        ..FakeRuntime::healthy()
    };
    // Another developer's run is alive on the same host.
    runtime
        .running
        .lock()
        .unwrap()
        .push("mortsom-relay-other".into());

    let err = start_relay(
        &runtime,
        &FakeProbe(false),
        &config(),
        "mine",
        dir.path(),
        Duration::from_millis(50),
    )
    .await
    .unwrap_err();

    match err {
        RelayError::NotReady {
            container_name,
            logs,
            ..
        } => {
            assert_eq!(container_name, "mortsom-relay-mine");
            assert_eq!(logs, "relay panicked");
        }
        other => panic!("unexpected {other:?}"),
    }
    assert_eq!(
        *runtime.removed.lock().unwrap(),
        vec!["mortsom-relay-mine".to_owned()]
    );
    assert!(runtime.is_running("mortsom-relay-other").await.unwrap());
}

/// Fails the first `run` with a port race, then succeeds.
struct RacyRuntime {
    inner: FakeRuntime,
    races_left: Mutex<u16>,
    ports_seen: Mutex<Vec<u16>>,
}

#[async_trait]
impl ContainerRuntime for RacyRuntime {
    async fn run(&self, spec: &ContainerSpec) -> Result<String, DockerError> {
        self.ports_seen.lock().unwrap().push(spec.ports[0].0);
        let race = {
            let mut left = self.races_left.lock().unwrap();
            let race = *left > 0;
            if race {
                *left -= 1;
            }
            race
        };
        if race {
            return Err(DockerError::Command {
                command: "run".into(),
                stderr: "Bind for 0.0.0.0:47000 failed: port is already allocated".into(),
            });
        }
        self.inner.run(spec).await
    }
    async fn is_running(&self, name: &str) -> Result<bool, DockerError> {
        self.inner.is_running(name).await
    }
    async fn stop_and_remove(&self, name: &str) -> Result<(), DockerError> {
        self.inner.stop_and_remove(name).await
    }
    async fn restart(&self, name: &str) -> Result<(), DockerError> {
        self.inner.restart(name).await
    }
    async fn logs(&self, name: &str) -> Result<String, DockerError> {
        self.inner.logs(name).await
    }
    async fn list_names(&self, prefix: &str) -> Result<Vec<String>, DockerError> {
        self.inner.list_names(prefix).await
    }
}

#[tokio::test]
async fn lost_port_race_retries_with_a_higher_port() {
    let dir = tempfile::tempdir().unwrap();
    let runtime = RacyRuntime {
        inner: FakeRuntime::healthy(),
        races_left: Mutex::new(2),
        ports_seen: Mutex::new(Vec::new()),
    };

    let handle = start_relay(
        &runtime,
        &FakeProbe(true),
        &config(),
        "race",
        dir.path(),
        Duration::from_secs(1),
    )
    .await
    .unwrap();

    let seen = runtime.ports_seen.lock().unwrap().clone();
    assert_eq!(seen.len(), 3, "{seen:?}");
    assert!(seen.windows(2).all(|w| w[1] > w[0]), "{seen:?}");
    assert_eq!(handle.host_port, seen[2]);
}

#[tokio::test]
async fn persistent_port_race_gives_up_with_no_free_port() {
    let dir = tempfile::tempdir().unwrap();
    let runtime = RacyRuntime {
        inner: FakeRuntime::healthy(),
        races_left: Mutex::new(u16::MAX),
        ports_seen: Mutex::new(Vec::new()),
    };

    let err = start_relay(
        &runtime,
        &FakeProbe(true),
        &config(),
        "race",
        dir.path(),
        Duration::from_secs(1),
    )
    .await
    .unwrap_err();

    assert!(matches!(err, RelayError::NoFreePort { base: 47000 }));
    assert_eq!(
        runtime.ports_seen.lock().unwrap().len(),
        usize::from(PORT_RACE_RETRIES) + 1
    );
}

#[tokio::test]
async fn docker_run_failure_removes_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let runtime = FakeRuntime {
        fail_run: true,
        ..FakeRuntime::healthy()
    };
    runtime
        .running
        .lock()
        .unwrap()
        .push("mortsom-relay-other".into());

    let err = start_relay(
        &runtime,
        &FakeProbe(true),
        &config(),
        "mine",
        dir.path(),
        Duration::from_secs(1),
    )
    .await
    .unwrap_err();

    assert!(matches!(err, RelayError::Docker(_)));
    assert!(runtime.removed.lock().unwrap().is_empty());
}

#[tokio::test]
async fn container_that_dies_during_wait_fails_fast() {
    let dir = tempfile::tempdir().unwrap();
    // `run` succeeds but the container is never observed running.
    let runtime = FakeRuntime::default();
    let started = std::time::Instant::now();

    let err = start_relay(
        &runtime,
        &FakeProbe(false),
        &config(),
        "x",
        dir.path(),
        Duration::from_secs(30),
    )
    .await
    .unwrap_err();

    assert!(matches!(err, RelayError::NotReady { .. }));
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "must not wait the full deadline"
    );
}

#[tokio::test]
async fn teardown_removes_container_and_data_unless_kept() {
    let dir = tempfile::tempdir().unwrap();
    let runtime = FakeRuntime {
        logs: "bye".into(),
        ..FakeRuntime::healthy()
    };
    let handle = start_relay(
        &runtime,
        &FakeProbe(true),
        &config(),
        "t",
        dir.path(),
        Duration::from_secs(1),
    )
    .await
    .unwrap();

    let (kept, logs) = teardown_relay(&runtime, &handle, true).await;
    assert!(kept.removed && !kept.data_removed && kept.error.is_none());
    assert_eq!(logs, "bye");
    assert!(handle.data_dir.is_dir());

    let (gone, _) = teardown_relay(&runtime, &handle, false).await;
    assert!(gone.removed && gone.data_removed);
    assert!(!handle.data_dir.exists());
}

#[tokio::test]
async fn restart_waits_for_readiness_again() {
    let dir = tempfile::tempdir().unwrap();
    let runtime = FakeRuntime::healthy();
    let handle = start_relay(
        &runtime,
        &FakeProbe(true),
        &config(),
        "r",
        dir.path(),
        Duration::from_secs(1),
    )
    .await
    .unwrap();

    restart_relay(&runtime, &FakeProbe(true), &handle, Duration::from_secs(1))
        .await
        .unwrap();
    let err = restart_relay(
        &runtime,
        &FakeProbe(false),
        &handle,
        Duration::from_millis(20),
    )
    .await
    .unwrap_err();

    assert!(matches!(err, RelayError::NotReady { .. }));
    assert!(
        runtime.is_running(&handle.container_name).await.unwrap(),
        "restart never removes the container"
    );
}
