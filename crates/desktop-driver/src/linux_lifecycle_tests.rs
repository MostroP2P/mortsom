#![allow(clippy::unwrap_used)]
use super::*;
use mortsom_android_driver::process::{Output, ProcessError};
use std::sync::{
    Mutex as StdMutex,
    atomic::{AtomicUsize, Ordering},
};

struct Child {
    attempts: Arc<AtomicUsize>,
}
#[async_trait]
impl ChildProcess for Child {
    async fn kill(&self) -> Result<(), ProcessError> {
        if self.attempts.fetch_add(1, Ordering::SeqCst) == 0 {
            Err(ProcessError::Spawn {
                command: "kill".into(),
                message: "injected stop failure".into(),
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
    root: StdMutex<Option<PathBuf>>,
    attempts: Arc<AtomicUsize>,
}
#[async_trait]
impl CommandRunner for Runner {
    async fn run(&self, _: &Command, _: Duration) -> Result<Output, ProcessError> {
        unreachable!()
    }
    async fn spawn(
        &self,
        command: &Command,
        log: Option<&Path>,
    ) -> Result<Box<dyn ChildProcess>, ProcessError> {
        if command.program == Path::new("Xvfb") {
            let log = log.unwrap();
            *self.root.lock().unwrap() = log.parent().map(Path::to_path_buf);
            std::fs::write(log, "7\n").unwrap();
            Ok(Box::new(Child {
                attempts: self.attempts.clone(),
            }))
        } else {
            Err(ProcessError::Spawn {
                command: command.display(),
                message: "injected bus startup failure".into(),
            })
        }
    }
}
#[tokio::test]
async fn failed_startup_keeps_failed_child_handles_and_files_until_retry() {
    let runner = Arc::new(Runner {
        root: StdMutex::new(None),
        attempts: Arc::new(AtomicUsize::new(0)),
    });
    let session = LinuxSession::new(runner.clone(), "app".into(), "Xvfb".into());
    assert!(session.clear_app().await.is_err());
    let root = runner.root.lock().unwrap().clone().unwrap();
    assert!(
        root.exists(),
        "storage cannot be deleted while an owned child survives"
    );
    session.end().await.unwrap();
    assert!(!root.exists());
    assert_eq!(runner.attempts.load(Ordering::SeqCst), 2);
}

struct InterruptedChild {
    attempts: Arc<AtomicUsize>,
}
#[async_trait]
impl ChildProcess for InterruptedChild {
    async fn kill(&self) -> Result<(), ProcessError> {
        if self.attempts.fetch_add(1, Ordering::SeqCst) == 0 {
            std::future::pending::<()>().await;
        }
        Ok(())
    }
    async fn is_alive(&self) -> bool {
        true
    }
}
#[tokio::test]
async fn cancelled_shutdown_keeps_all_service_handles_for_the_next_attempt() {
    let root = tempfile::tempdir().unwrap();
    let attempts = Arc::new(AtomicUsize::new(0));
    let session = LinuxSession::new(
        Arc::new(mortsom_android_driver::process::TokioRunner),
        "app".into(),
        "Xvfb".into(),
    );
    *session.runtime.lock().await = Some(Runtime {
        root: root.path().to_path_buf(),
        env: Default::default(),
        native: None,
        app: None,
        services: vec![Box::new(InterruptedChild {
            attempts: attempts.clone(),
        })],
    });
    assert!(
        tokio::time::timeout(Duration::from_millis(10), session.end())
            .await
            .is_err()
    );
    assert_eq!(
        session
            .runtime
            .lock()
            .await
            .as_ref()
            .unwrap()
            .services
            .len(),
        1
    );
    assert!(root.path().exists());
    session.end().await.unwrap();
    assert!(!root.path().exists());
    assert_eq!(attempts.load(Ordering::SeqCst), 2);
}
#[tokio::test]
async fn failed_app_stop_retains_its_handle_before_a_retry() {
    let root = tempfile::tempdir().unwrap();
    let attempts = Arc::new(AtomicUsize::new(0));
    let session = LinuxSession::new(
        Arc::new(mortsom_android_driver::process::TokioRunner),
        "app".into(),
        "Xvfb".into(),
    );
    *session.runtime.lock().await = Some(Runtime {
        root: root.path().to_path_buf(),
        env: Default::default(),
        native: None,
        app: Some(Box::new(Child {
            attempts: attempts.clone(),
        })),
        services: Vec::new(),
    });
    assert!(session.terminate_app().await.is_err());
    assert!(session.runtime.lock().await.as_ref().unwrap().app.is_some());
    session.terminate_app().await.unwrap();
    assert!(session.runtime.lock().await.as_ref().unwrap().app.is_none());
    session.end().await.unwrap();
}
