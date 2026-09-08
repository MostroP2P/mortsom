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
