#![allow(clippy::unwrap_used)]
use super::*;
use mortsom_ui_driver::process::{Output, ProcessError};
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
    protected: Option<PathBuf>,
    root: StdMutex<Option<PathBuf>>,
    env: StdMutex<Option<std::collections::BTreeMap<String, String>>>,
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
        if let Some(path) = &self.protected {
            assert!(
                crate::storage::ensure_stopped(path).is_err(),
                "durable marker must precede every child spawn"
            );
        }
        if command.program == Path::new("Xvfb") {
            let log = log.unwrap();
            *self.root.lock().unwrap() = log.parent().map(Path::to_path_buf);
            std::fs::write(log, "7\n").unwrap();
            Ok(Box::new(Child {
                attempts: self.attempts.clone(),
            }))
        } else {
            *self.env.lock().unwrap() = Some(command.env.clone());
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
        protected: None,
        root: StdMutex::new(None),
        env: StdMutex::new(None),
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
    let storage_parent = tempfile::tempdir().unwrap();
    let storage = storage_parent.path().join("alice");
    crate::storage::ActorStorage::Fresh(storage.clone())
        .prepare(crate::Platform::Linux)
        .unwrap();
    let attempts = Arc::new(AtomicUsize::new(0));
    let session = LinuxSession::new(
        Arc::new(mortsom_ui_driver::process::TokioRunner),
        "app".into(),
        "Xvfb".into(),
    );
    *session.runtime.lock().await = Some(Runtime {
        active: Some(crate::storage::ActiveRuntime::begin(&storage).unwrap()),
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
    assert!(crate::storage::ensure_stopped(&storage).is_err());
    session.end().await.unwrap();
    crate::storage::ensure_stopped(&storage).unwrap();
    assert!(!root.path().exists());
    assert_eq!(attempts.load(Ordering::SeqCst), 2);
}
#[tokio::test]
async fn failed_app_stop_retains_its_handle_before_a_retry() {
    let root = tempfile::tempdir().unwrap();
    let attempts = Arc::new(AtomicUsize::new(0));
    let session = LinuxSession::new(
        Arc::new(mortsom_ui_driver::process::TokioRunner),
        "app".into(),
        "Xvfb".into(),
    );
    *session.runtime.lock().await = Some(Runtime {
        active: None,
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

#[tokio::test]
async fn failed_linux_start_and_end_retain_protected_data_for_resume() {
    use crate::storage::ActorStorage;
    let parent = tempfile::tempdir().unwrap();
    let path = parent.path().join("alice");
    let runner = Arc::new(Runner {
        protected: Some(path.clone()),
        root: StdMutex::new(None),
        env: StdMutex::new(None),
        attempts: Arc::new(AtomicUsize::new(0)),
    });
    let fresh = LinuxSession::new(runner.clone(), "app".into(), "Xvfb".into())
        .with_storage(ActorStorage::Fresh(path.clone()));
    assert!(fresh.clear_app().await.is_err());
    assert!(
        path.join(".mortsom-active-runtime").exists(),
        "failed child shutdown must retain its durable marker"
    );
    {
        let env = runner.env.lock().unwrap();
        let env = env.as_ref().unwrap();
        assert_eq!(
            env["XDG_DATA_HOME"],
            path.join("data").display().to_string()
        );
        assert_eq!(
            env["XDG_CONFIG_HOME"],
            path.join("config").display().to_string()
        );
        assert_eq!(
            env["XDG_RUNTIME_DIR"],
            runner
                .root
                .lock()
                .unwrap()
                .as_ref()
                .unwrap()
                .display()
                .to_string()
        );
        assert_ne!(env["XDG_RUNTIME_DIR"], path.display().to_string());
    }
    std::fs::write(path.join("data/identity-placeholder"), b"retained identity").unwrap();
    fresh.end().await.unwrap();
    assert!(!path.join(".mortsom-active-runtime").exists());
    assert!(!runner.root.lock().unwrap().as_ref().unwrap().exists());
    assert!(fresh.clear_app().await.is_err());
    let resumed = LinuxSession::new(runner.clone(), "app".into(), "Xvfb".into())
        .with_storage(ActorStorage::Resume(path.clone()));
    assert!(resumed.clear_app().await.is_err());
    let before = runner.root.lock().unwrap().clone();
    // The injected bus failure occurs only after resume validated existing data
    // and launched a new display; no real D-Bus session is needed in this test.
    assert!(resumed.activate_app().await.is_err());
    assert_ne!(*runner.root.lock().unwrap(), before);
    resumed.end().await.unwrap();
    assert!(!runner.root.lock().unwrap().as_ref().unwrap().exists());
    assert_eq!(
        std::fs::read(path.join("data/identity-placeholder")).unwrap(),
        b"retained identity"
    );
}

#[test]
fn private_buses_run_on_dbus_daemon_with_their_own_socket_and_type() {
    let root = tempfile::tempdir().unwrap();
    let env = std::collections::BTreeMap::from([("DISPLAY".to_string(), ":7".to_string())]);
    let (command, address) = private_bus(root.path(), "a11y", "accessibility", &env).unwrap();
    assert_eq!(command.program, Path::new("dbus-daemon"));
    assert_eq!(command.env, env);
    let socket = root.path().join("a11y");
    assert_eq!(address, format!("unix:path={}", socket.display()));
    let config = std::fs::read_to_string(root.path().join("a11y.conf")).unwrap();
    assert!(config.contains("<type>accessibility</type>"));
    assert!(config.contains(&format!("<listen>{address}</listen>")));
    assert!(command.args.contains(&format!(
        "--config-file={}",
        root.path().join("a11y.conf").display()
    )));
}

#[test]
fn the_accessibility_bus_never_depends_on_the_host_bus_launcher() {
    // at-spi-bus-launcher picks dbus-broker whenever it runs inside a systemd
    // user unit (a GNOME terminal, for one), and dbus-broker cannot start on
    // a private session bus that has no systemd behind it.
    let root = tempfile::tempdir().unwrap();
    let (command, _) =
        private_bus(root.path(), "a11y", "accessibility", &Default::default()).unwrap();
    assert!(!command.display().contains("at-spi-bus-launcher"));
    assert!(!command.display().contains("dbus-broker"));
}
