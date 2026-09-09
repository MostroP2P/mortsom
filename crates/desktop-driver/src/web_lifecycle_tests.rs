#![allow(clippy::unwrap_used, clippy::expect_used)]
use super::*;
use mortsom_android_driver::process::{Output, ProcessError};
use std::sync::Mutex as StdMutex;

#[derive(Default)]
struct MissingDriver {
    root: StdMutex<Option<PathBuf>>,
}
#[async_trait]
impl CommandRunner for MissingDriver {
    async fn run(&self, _: &Command, _: Duration) -> Result<Output, ProcessError> {
        unreachable!()
    }
    async fn spawn(
        &self,
        command: &Command,
        log: Option<&std::path::Path>,
    ) -> Result<Box<dyn ChildProcess>, ProcessError> {
        if let Some(log) = log {
            *self.root.lock().unwrap() = log.parent().map(std::path::Path::to_path_buf);
        }
        Err(ProcessError::Spawn {
            command: command.display(),
            message: "missing driver".into(),
        })
    }
}
#[tokio::test]
async fn missing_driver_does_not_leave_a_profile_or_runtime_directory() {
    let runner = Arc::new(MissingDriver::default());
    let session = WebSession::new(
        runner.clone(),
        "http://127.0.0.1:9000".into(),
        "missing-driver".into(),
        "browser".into(),
    );
    assert!(session.clear_app().await.is_err());
    let root = runner
        .root
        .lock()
        .unwrap()
        .clone()
        .expect("startup must capture driver output in its private directory");
    assert!(!root.exists());
    session.end().await.unwrap();
}

struct Child {
    attempts: Arc<std::sync::atomic::AtomicUsize>,
    fail_once: bool,
}
#[async_trait]
impl ChildProcess for Child {
    async fn kill(&self) -> Result<(), ProcessError> {
        use std::sync::atomic::Ordering;
        if self.attempts.fetch_add(1, Ordering::SeqCst) == 0 && self.fail_once {
            Err(ProcessError::Spawn {
                command: "kill".into(),
                message: "injected failure".into(),
            })
        } else {
            Ok(())
        }
    }
    async fn is_alive(&self) -> bool {
        true
    }
}
struct Runner {
    port: u16,
    root: StdMutex<Option<PathBuf>>,
    attempts: Arc<std::sync::atomic::AtomicUsize>,
    fail_once: bool,
}
#[async_trait]
impl CommandRunner for Runner {
    async fn run(&self, _: &Command, _: Duration) -> Result<Output, ProcessError> {
        unreachable!()
    }
    async fn spawn(
        &self,
        command: &Command,
        log: Option<&std::path::Path>,
    ) -> Result<Box<dyn ChildProcess>, ProcessError> {
        assert!(command.args.contains(&"--port=0".into()));
        let log = log.unwrap();
        *self.root.lock().unwrap() = log.parent().map(std::path::Path::to_path_buf);
        std::fs::write(
            log,
            format!(
                "ChromeDriver was started successfully on port {}.\n",
                self.port
            ),
        )
        .unwrap();
        Ok(Box::new(Child {
            attempts: self.attempts.clone(),
            fail_once: self.fail_once,
        }))
    }
}

async fn failed_start(navigation: bool, fail_once: bool) {
    use wiremock::{
        Mock, MockServer, ResponseTemplate,
        matchers::{method, path},
    };
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/session"))
        .respond_with(if navigation {
            ResponseTemplate::new(200).set_body_json(json!({"value":{"sessionId":"owned"}}))
        } else {
            ResponseTemplate::new(500).set_body_json(
                json!({"value":{"error":"session not created","message":"injected"}}),
            )
        })
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/session/owned/url"))
        .respond_with(ResponseTemplate::new(500).set_body_json(
            json!({"value":{"error":"unknown error","message":"navigation failed"}}),
        ))
        .mount(&server)
        .await;
    Mock::given(method("DELETE"))
        .and(path("/session/owned"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"value":null})))
        .expect(u64::from(navigation))
        .mount(&server)
        .await;
    let runner = Arc::new(Runner {
        port: server.address().port(),
        root: StdMutex::new(None),
        attempts: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        fail_once,
    });
    let session = WebSession::new(
        runner.clone(),
        "http://127.0.0.1:9000".into(),
        "driver".into(),
        "browser".into(),
    );
    assert!(session.clear_app().await.is_err());
    let root = runner.root.lock().unwrap().clone().unwrap();
    assert_eq!(root.exists(), fail_once);
    assert_eq!(session.runtime.lock().await.is_some(), fail_once);
    session.end().await.unwrap();
    assert!(!root.exists());
    assert_eq!(
        runner.attempts.load(std::sync::atomic::Ordering::SeqCst),
        if fail_once { 2 } else { 1 }
    );
    server.verify().await;
}
#[tokio::test]
async fn failed_session_creation_releases_driver_and_profile() {
    failed_start(false, false).await;
}
#[tokio::test]
async fn failed_navigation_deletes_session_then_stops_driver_and_profile() {
    failed_start(true, false).await;
}
#[tokio::test]
async fn failed_driver_stop_retains_ownership_for_an_explicit_retry() {
    failed_start(true, true).await;
}

#[tokio::test]
async fn persistent_profile_survives_end_and_resumes_without_clear_or_new_identity() {
    use crate::storage::ActorStorage;
    use wiremock::{
        Mock, MockServer, ResponseTemplate,
        matchers::{method, path},
    };
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/session"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({"value":{"sessionId":"owned"}})),
        )
        .expect(2)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/session/owned/url"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"value":null})))
        .expect(2)
        .mount(&server)
        .await;
    Mock::given(method("DELETE"))
        .and(path("/session/owned"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"value":null})))
        .expect(2)
        .mount(&server)
        .await;
    let runner = Arc::new(Runner {
        port: server.address().port(),
        root: StdMutex::new(None),
        attempts: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        fail_once: false,
    });
    let parent = tempfile::tempdir().unwrap();
    let path = parent.path().join("alice");
    let make_session = |storage| {
        WebSession::new(
            runner.clone(),
            "http://127.0.0.1:9000".into(),
            "driver".into(),
            "browser".into(),
        )
        .with_storage(storage)
    };
    let fresh = make_session(ActorStorage::Fresh(path.clone()));
    fresh.clear_app().await.unwrap();
    assert!(path.join(".mortsom-active-runtime").exists());
    let original_runtime = runner.root.lock().unwrap().clone().unwrap();
    let identity = path.join("profile/identity-placeholder");
    std::fs::write(&identity, b"retained identity").unwrap();
    fresh.end().await.unwrap();
    assert!(!path.join(".mortsom-active-runtime").exists());
    assert!(!original_runtime.exists());
    assert!(fresh.clear_app().await.is_err());
    let resumed = make_session(ActorStorage::Resume(path.clone()));
    assert!(resumed.clear_app().await.is_err());
    resumed.activate_app().await.unwrap();
    resumed.end().await.unwrap();
    assert!(!runner.root.lock().unwrap().as_ref().unwrap().exists());
    assert_eq!(std::fs::read(identity).unwrap(), b"retained identity");
    let requests = server.received_requests().await.unwrap();
    for request in requests
        .iter()
        .filter(|request| request.url.path() == "/session")
    {
        let body: serde_json::Value = serde_json::from_slice(&request.body).unwrap();
        let args = body["capabilities"]["alwaysMatch"]["goog:chromeOptions"]["args"]
            .as_array()
            .unwrap();
        assert!(args.contains(&json!(format!(
            "--user-data-dir={}",
            path.join("profile").display()
        ))));
    }
    server.verify().await;
}

#[tokio::test]
async fn absent_resume_storage_fails_before_starting_a_browser() {
    let runner = Arc::new(MissingDriver::default());
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("missing");
    let session = WebSession::new(
        runner.clone(),
        "http://127.0.0.1:9000".into(),
        "driver".into(),
        "browser".into(),
    )
    .with_storage(crate::storage::ActorStorage::Resume(path.clone()));
    assert!(session.activate_app().await.is_err());
    assert!(runner.root.lock().unwrap().is_none());
    assert!(!path.exists());
}

#[tokio::test]
async fn crash_marker_refuses_resume_before_any_browser_process_starts() {
    let runner = Arc::new(MissingDriver::default());
    let parent = tempfile::tempdir().unwrap();
    let path = parent.path().join("alice");
    crate::storage::ActorStorage::Fresh(path.clone())
        .prepare(crate::Platform::Web)
        .unwrap();
    drop(crate::storage::ActiveRuntime::begin(&path).unwrap());
    let session = WebSession::new(
        runner.clone(),
        "http://127.0.0.1:9000".into(),
        "driver".into(),
        "browser".into(),
    )
    .with_storage(crate::storage::ActorStorage::Resume(path.clone()));
    assert!(
        session
            .activate_app()
            .await
            .unwrap_err()
            .to_string()
            .contains("unverified prior shutdown")
    );
    assert!(runner.root.lock().unwrap().is_none());
    session.end().await.unwrap();
    assert!(
        crate::storage::ensure_stopped(&path).is_err(),
        "a new session must not clear an old runtime's marker"
    );
}
