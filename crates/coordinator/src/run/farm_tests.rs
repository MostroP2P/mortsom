#![allow(clippy::unwrap_used)]
use super::*;
use mortsom_ui_driver::ui::tests_support::ScriptedUi;

#[tokio::test]
async fn failed_actor_shutdown_is_retried_before_releasing_session_ownership() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("asset"), b"fixture").unwrap();
    let bundle = Arc::new(crate::fingerprint::CapturedBundle::capture(root.path()).unwrap());
    let alice = Arc::new(ScriptedUi::default());
    alice.fail_end_times(1);
    let bob = Arc::new(ScriptedUi::default());
    let farm = Farm::Desktop {
        sessions: vec![(Actor::Alice, alice.clone()), (Actor::Bob, bob.clone())],
        server: None,
        bundle,
        app_version: None,
    };
    let result = farm.tear_down().await;
    assert!(result.iter().all(|item| item.ok));
    assert!(alice.calls().contains(&"end failed".to_owned()));
    assert!(alice.calls().contains(&"end session".to_owned()));
    assert!(bob.calls().contains(&"end session".to_owned()));
}

struct Server {
    stopped: Arc<std::sync::atomic::AtomicBool>,
}
#[async_trait::async_trait]
impl DesktopServer for Server {
    fn url(&self) -> &str {
        "http://127.0.0.1:9000"
    }
    async fn stop(&mut self) -> Result<(), String> {
        self.stopped
            .store(true, std::sync::atomic::Ordering::SeqCst);
        Ok(())
    }
}
struct Factory {
    alice: Arc<ScriptedUi>,
    bob: Arc<ScriptedUi>,
    fail_bob: bool,
    stopped: Arc<std::sync::atomic::AtomicBool>,
}
#[async_trait::async_trait]
impl DesktopFactory for Factory {
    async fn server(&self, _: &DesktopOptions) -> Result<Box<dyn DesktopServer>, String> {
        Ok(Box::new(Server {
            stopped: self.stopped.clone(),
        }))
    }
    fn session(
        &self,
        actor: Actor,
        _: &DesktopOptions,
        origin: Option<&str>,
    ) -> Result<Arc<dyn UiSession>, String> {
        assert_eq!(origin, Some("http://127.0.0.1:9000"));
        match actor {
            Actor::Alice => Ok(self.alice.clone()),
            Actor::Bob if self.fail_bob => Err("injected second actor failure".into()),
            Actor::Bob => Ok(self.bob.clone()),
        }
    }
}
fn fixture() -> (DesktopOptions, Factory) {
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir(root.path().join("pkg")).unwrap();
    for file in [
        "index.html",
        "flutter_bootstrap.js",
        "main.dart.js",
        "pkg/rust.js",
        "pkg/rust_bg.wasm",
    ] {
        std::fs::write(root.path().join(file), b"fixture").unwrap();
    }
    let bundle = Arc::new(crate::fingerprint::CapturedBundle::capture(root.path()).unwrap());
    let options = DesktopOptions {
        recovery: None,
        resume: false,
        artifact: bundle.root().into(),
        bundle,
        platform: Platform::Web,
        provenance: crate::provenance::BuildProvenance::prebuilt(),
        xvfb: "missing".into(),
        browser: "missing".into(),
        relay_ruled_out: Vec::new(),
        chromedriver: "missing".into(),
    };
    let factory = Factory {
        alice: Arc::default(),
        bob: Arc::default(),
        fail_bob: false,
        stopped: Arc::new(std::sync::atomic::AtomicBool::new(false)),
    };
    (options, factory)
}
#[tokio::test]
async fn second_actor_construction_failure_closes_the_first_actor_and_owned_server() {
    let (options, mut factory) = fixture();
    factory.fail_bob = true;
    assert!(Farm::desktop(&options, &factory).await.is_err());
    assert!(factory.alice.calls().contains(&"end session".to_owned()));
    assert!(factory.bob.calls().is_empty());
    assert!(factory.stopped.load(std::sync::atomic::Ordering::SeqCst));
}
#[tokio::test]
async fn injected_actors_are_independent_and_shutdown_releases_the_shared_server() {
    let (options, factory) = fixture();
    let farm = Farm::desktop(&options, &factory).await.unwrap();
    let sessions = farm.sessions();
    assert!(!Arc::ptr_eq(&sessions[0].1, &sessions[1].1));
    assert!(factory.alice.calls().is_empty() && factory.bob.calls().is_empty());
    let report = farm.tear_down().await;
    assert!(report.iter().all(|item| item.ok));
    assert!(factory.stopped.load(std::sync::atomic::Ordering::SeqCst));
}
#[tokio::test]
async fn exhausted_shutdown_retries_still_close_the_sibling_and_server_and_report_failure() {
    let (options, factory) = fixture();
    factory.alice.fail_end_times(2);
    let farm = Farm::desktop(&options, &factory).await.unwrap();
    let report = farm.tear_down().await;
    assert!(!report[0].ok);
    assert!(report[0].error.as_deref().unwrap().contains("retry"));
    assert!(report[1].ok);
    assert!(factory.bob.calls().contains(&"end session".to_owned()));
    assert!(factory.stopped.load(std::sync::atomic::Ordering::SeqCst));
}
