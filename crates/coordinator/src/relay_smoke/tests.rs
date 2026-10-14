#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::Mutex;

use mortsom_environment::docker::{ContainerSpec, DockerError};

use super::*;

const HEX: &str = "8171eb680049f62ce8747d203f57c3f71de6eea308af6f995363896a1e47cc51";
const ALICE: &str = "nostr+walletconnect://b889ff5b1513b641e2a139f661a661364979c5beee91842f8f0ef42ab558e9d4?relay=wss%3A%2F%2Frelay.damus.io&secret=71a8c14c1407c113601079c4302dab36460f0ccd0ad506f1f2dc73b5100e4f3c";
const BOB: &str = "nostr+walletconnect://b889ff5b1513b641e2a139f661a661364979c5beee91842f8f0ef42ab558e9d4?relay=wss%3A%2F%2Frelay.damus.io&secret=0000000000000000000000000000000000000000000000000000000000000001";

fn config(artifacts_dir: &std::path::Path) -> Config {
    let text = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../mortsom.toml.example"
    ))
    .unwrap()
    .replace("health_timeout_secs = 120", "health_timeout_secs = 1")
    .replace("host_port = 7000", "host_port = 47200")
    .replace(
        "artifacts_dir = \"./artifacts\"",
        &format!("artifacts_dir = \"{}\"", artifacts_dir.display()),
    );
    Config::from_toml_str(&text, |name| match name {
        "MORTSOM_MOSTRO_PUBKEY" => Some(HEX.into()),
        "MORTSOM_MOSTRO_SETTINGS_FILE" => Some("/tmp/settings.toml".into()),
        "MORTSOM_ALICE_NWC" => Some(ALICE.into()),
        "MORTSOM_BOB_NWC" => Some(BOB.into()),
        _ => None,
    })
    .unwrap()
}

#[derive(Default)]
struct FakeRuntime {
    running: Mutex<Vec<String>>,
    removed: Mutex<Vec<String>>,
}

#[async_trait]
impl ContainerRuntime for FakeRuntime {
    async fn run(&self, spec: &ContainerSpec) -> Result<String, DockerError> {
        self.running.lock().unwrap().push(spec.name.clone());
        Ok("id".into())
    }
    async fn is_running(&self, name: &str) -> Result<bool, DockerError> {
        Ok(self.running.lock().unwrap().iter().any(|n| n == name))
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
        Ok(format!("relay log with secret {ALICE}\n"))
    }
    async fn list_names(&self, _prefix: &str) -> Result<Vec<String>, DockerError> {
        Ok(self.running.lock().unwrap().clone())
    }
}

struct AlwaysReady;
#[async_trait]
impl ReadinessProbe for AlwaysReady {
    async fn is_ready(&self, _url: &str) -> bool {
        true
    }
}

struct ScriptedEvidence {
    events: Vec<ObservedEvent>,
    fail_start: bool,
}

struct ScriptedSession(Vec<ObservedEvent>);

#[async_trait]
impl EvidenceSource for ScriptedEvidence {
    async fn start(
        &self,
        _relay_url: &str,
        _pubkey: PublicKey,
    ) -> Result<Box<dyn EvidenceSession>, String> {
        if self.fail_start {
            return Err("cannot connect".into());
        }
        Ok(Box::new(ScriptedSession(self.events.clone())))
    }
}

#[async_trait]
impl EvidenceSession for ScriptedSession {
    async fn wait_for_kind(&self, kind: u16, _timeout: Duration) -> Option<ObservedEvent> {
        self.0.iter().find(|e| e.kind == kind).cloned()
    }
    fn snapshot(&self) -> Vec<ObservedEvent> {
        self.0.clone()
    }
    async fn stop(&self) {}
}

fn event(kind: u16, tags: &[&[&str]]) -> ObservedEvent {
    ObservedEvent {
        seq: 1,
        received_at_ms: 0,
        relay: "ws://127.0.0.1:47200".into(),
        id: format!("{kind:064x}"),
        kind,
        author: HEX.into(),
        created_at: 1,
        tags: tags
            .iter()
            .map(|t| t.iter().map(|s| (*s).to_owned()).collect())
            .collect(),
        content: Some(String::new()),
        raw: None,
        action: None,
        query: false,
    }
}

fn relay_list_event(url: &str) -> ObservedEvent {
    event(10002, &[&["r", url]])
}

#[tokio::test]
async fn healthy_smoke_passes_writes_artifacts_and_tears_down() {
    let dir = tempfile::tempdir().unwrap();
    let config = config(dir.path());
    let runtime = FakeRuntime::default();
    let evidence = ScriptedEvidence {
        events: vec![
            relay_list_event("ws://host.docker.internal:7000"),
            event(
                NOSTR_INFO_EVENT_KIND,
                &[&["protocol_version", "2"], &["secret", ALICE]],
            ),
        ],
        fail_start: false,
    };

    let summary = run_relay_smoke(
        &config,
        &SmokeOptions::default(),
        &runtime,
        &AlwaysReady,
        &evidence,
    )
    .await
    .unwrap();

    assert!(summary.passed, "{}", summary.checks.render());
    assert!(
        runtime.running.lock().unwrap().is_empty(),
        "container torn down"
    );
    let run_dir = dir.path().join(summary.run_id.as_str());
    for name in [
        files::SUMMARY,
        files::ENVIRONMENT,
        files::TIMELINE,
        files::RELAY_EVENTS,
        files::CLEANUP,
        "relay-container.log",
        "run.log",
    ] {
        assert!(run_dir.join(name).is_file(), "{name}");
    }
    let all = std::fs::read_dir(&run_dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.path().is_file())
        .map(|e| std::fs::read_to_string(e.path()).unwrap())
        .collect::<String>();
    assert!(!all.contains("secret=71a8"), "secret leaked into artifacts");
    assert_eq!(
        std::fs::read_to_string(run_dir.join(files::RELAY_EVENTS))
            .unwrap()
            .lines()
            .count(),
        2
    );
}

#[tokio::test]
async fn missing_relay_list_fails_but_still_tears_down_unless_kept() {
    let dir = tempfile::tempdir().unwrap();
    let config = config(dir.path());
    let runtime = FakeRuntime::default();
    let evidence = ScriptedEvidence {
        events: vec![],
        fail_start: false,
    };

    let summary = run_relay_smoke(
        &config,
        &SmokeOptions::default(),
        &runtime,
        &AlwaysReady,
        &evidence,
    )
    .await
    .unwrap();
    assert!(!summary.passed);
    assert!(runtime.running.lock().unwrap().is_empty());
    assert!(
        summary
            .checks
            .checks
            .iter()
            .any(|c| c.name == "daemon-uses-run-relay" && c.status == CheckStatus::Fail)
    );

    let kept = run_relay_smoke(
        &config,
        &SmokeOptions {
            keep_on_failure: true,
        },
        &runtime,
        &AlwaysReady,
        &evidence,
    )
    .await
    .unwrap();
    assert!(!kept.passed);
    let teardown = kept.teardown.as_ref().unwrap();
    assert!(teardown.removed, "container removed, data kept");
    assert!(!teardown.data_removed);
}

#[tokio::test]
async fn relay_list_without_the_run_relay_fails() {
    let dir = tempfile::tempdir().unwrap();
    let config = config(dir.path());
    let runtime = FakeRuntime::default();
    let evidence = ScriptedEvidence {
        events: vec![relay_list_event("wss://relay.mostro.network")],
        fail_start: false,
    };

    let summary = run_relay_smoke(
        &config,
        &SmokeOptions::default(),
        &runtime,
        &AlwaysReady,
        &evidence,
    )
    .await
    .unwrap();

    assert!(!summary.passed);
    assert!(
        summary
            .checks
            .checks
            .iter()
            .any(|c| c.name == "daemon-uses-run-relay" && c.detail.contains("none of"))
    );
}

#[tokio::test]
async fn observer_failure_and_relay_start_failure_are_reported() {
    let dir = tempfile::tempdir().unwrap();
    let config = config(dir.path());
    let runtime = FakeRuntime::default();
    let evidence = ScriptedEvidence {
        events: vec![],
        fail_start: true,
    };
    let summary = run_relay_smoke(
        &config,
        &SmokeOptions::default(),
        &runtime,
        &AlwaysReady,
        &evidence,
    )
    .await
    .unwrap();
    assert!(
        summary
            .checks
            .checks
            .iter()
            .any(|c| c.name == "relay-observer" && c.status == CheckStatus::Fail)
    );

    struct NeverReady;
    #[async_trait]
    impl ReadinessProbe for NeverReady {
        async fn is_ready(&self, _url: &str) -> bool {
            false
        }
    }
    let summary = run_relay_smoke(
        &config,
        &SmokeOptions::default(),
        &runtime,
        &NeverReady,
        &evidence,
    )
    .await
    .unwrap();
    assert!(!summary.passed);
    assert!(summary.relay.is_none());
    assert!(
        summary
            .checks
            .checks
            .iter()
            .any(|c| c.name == "relay-start" && c.status == CheckStatus::Fail)
    );
}

#[tokio::test]
async fn a_panicking_observer_still_tears_down_the_relay() {
    struct PanickingEvidence;
    struct PanickingSession;
    #[async_trait]
    impl EvidenceSource for PanickingEvidence {
        async fn start(
            &self,
            _relay_url: &str,
            _pubkey: PublicKey,
        ) -> Result<Box<dyn EvidenceSession>, String> {
            Ok(Box::new(PanickingSession))
        }
    }
    #[async_trait]
    impl EvidenceSession for PanickingSession {
        async fn wait_for_kind(&self, _kind: u16, _timeout: Duration) -> Option<ObservedEvent> {
            panic!("boom");
        }
        fn snapshot(&self) -> Vec<ObservedEvent> {
            Vec::new()
        }
        async fn stop(&self) {}
    }
    let dir = tempfile::tempdir().unwrap();
    let config = config(dir.path());
    let runtime = FakeRuntime::default();

    let summary = run_relay_smoke(
        &config,
        &SmokeOptions::default(),
        &runtime,
        &AlwaysReady,
        &PanickingEvidence,
    )
    .await
    .unwrap();

    assert!(!summary.passed);
    assert!(
        runtime.running.lock().unwrap().is_empty(),
        "relay torn down after panic"
    );
    assert!(summary.teardown.as_ref().unwrap().removed);
}

/// The message a failed check prints is the operator's whole diagnosis, so
/// the same relay must not appear twice under different spellings: the
/// configured daemon view and this run's own view are usually one URL.
#[test]
fn the_expected_relay_list_names_each_relay_once() {
    let dir = tempfile::tempdir().unwrap();
    let mut config = config(dir.path());
    config.mostro.relays = vec![
        "ws://host.docker.internal:47200".to_owned(),
        // Same endpoint under the runner's own name.
        "ws://localhost:47200".to_owned(),
        "wss://relay.mostro.network".to_owned(),
    ];
    let handle = RelayHandle {
        container_name: "mortsom-relay-test".to_owned(),
        host_port: 47200,
        data_dir: dir.path().to_path_buf(),
        image: "img".to_owned(),
    };

    let expected = expected_relay_urls(&config, &handle);

    assert_eq!(
        expected.len(),
        2,
        "the run relay and the public one, no repeats: {expected:?}"
    );
    assert!(
        expected.iter().any(|u| u.contains("relay.mostro.network")),
        "{expected:?}"
    );
    assert!(
        expected
            .iter()
            .any(|u| mortsom_environment::relay_url::same_relay(u, &handle.host_url())),
        "the run relay must still be there: {expected:?}"
    );
}
