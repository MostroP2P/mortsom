//! One actor's mostro-cli: its private storage, its serialized invocations
//! and their sanitized transcript.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use tokio::sync::OwnedMutexGuard;

use mortsom_desktop_driver::Platform;
use mortsom_desktop_driver::storage::{self, ActiveRuntime, ActorStorage};

use crate::call::{Call, NodeTarget};
use crate::envelope::Envelope;
use crate::error::CliError;
use crate::evidence::Evidence;
use crate::process::{CliProcess, ProcessRequest};
/// Beyond an invocation's own deadline, how long ending it may take: the
/// child is killed and reaped, its standard error read and its stop
/// recorded.
const SETTLE_GRACE: Duration = Duration::from_secs(5);

/// What every invocation of one actor shares.
#[derive(Debug, Clone)]
pub struct CliConfig {
    /// The mostro-cli executable.
    pub binary: PathBuf,
    /// The daemon and relays node commands talk to.
    pub node: NodeTarget,
    /// Deadline of one invocation; longer than the CLI's own deadlines.
    pub timeout: Duration,
    /// `PATH` handed to the child, if any. Only for a stand-in executable
    /// that needs one (a test script): the contract gives mostro-cli only
    /// `MCLI_DIR` and `HOME`, so a real run leaves it `None`.
    pub path_env: Option<String>,
}

/// Clears the marker of an interrupted runtime once no process uses the
/// actor's store: [`crate::runtime::reconcile`], which scans `/proc` and so
/// may block.
type Reconcile = Arc<dyn Fn(&Path) -> Result<(), CliError> + Send + Sync>;

/// One actor's command-line client.
pub struct CliClient {
    config: CliConfig,
    process: Arc<dyn CliProcess>,
    storage: Option<ActorStorage>,
    store: Mutex<Store>,
    /// Transcript and standard error, shared with running invocations.
    evidence: Evidence,
    /// Held for the whole life of an invocation, by the task that runs it,
    /// and of a reconciliation, by the blocking task that runs it: a caller
    /// that gives up never releases it while either still runs.
    serial: Arc<tokio::sync::Mutex<()>>,
    /// Run on the blocking pool, never on a thread of the runtime.
    reconcile: Reconcile,
}

/// The actor's storage as the client holds it.
#[derive(Default)]
struct Store {
    root: Option<Root>,
    /// Set by `close`: retained storage stays on disk for its owner, but
    /// nothing runs on it until the client is opened again.
    closed: bool,
}

impl Store {
    /// The root invocations run in: open, and not closed since.
    fn open_root(&self) -> Option<&Root> {
        self.root.as_ref().filter(|_| !self.closed)
    }
}

/// Where the actor's `MCLI_DIR` and `HOME` live.
enum Root {
    /// Caller-owned storage, retained for recovery.
    Retained(PathBuf),
    /// A private directory removed with the client.
    Temporary(tempfile::TempDir),
}

impl Root {
    fn path(&self) -> &Path {
        match self {
            Root::Retained(path) => path,
            Root::Temporary(dir) => dir.path(),
        }
    }
}

impl CliClient {
    /// A client whose storage is a private temporary directory.
    pub fn new(config: CliConfig, process: Arc<dyn CliProcess>) -> Self {
        Self {
            config,
            process,
            storage: None,
            store: Mutex::new(Store::default()),
            evidence: Evidence::default(),
            serial: Arc::new(tokio::sync::Mutex::new(())),
            reconcile: Arc::new(crate::runtime::reconcile),
        }
    }

    /// The same client, reconciling an interrupted runtime with `reconcile`.
    #[cfg(test)]
    fn reconciling_with(
        mut self,
        reconcile: impl Fn(&Path) -> Result<(), CliError> + Send + Sync + 'static,
    ) -> Self {
        self.reconcile = Arc::new(reconcile);
        self
    }

    /// Refuses a lifecycle change while an invocation, or a reconciliation,
    /// still runs.
    fn idle(&self) -> Result<OwnedMutexGuard<()>, CliError> {
        Arc::clone(&self.serial).try_lock_owned().map_err(|_| {
            CliError::Storage("an invocation or a reconciliation is still running".into())
        })
    }

    /// Keeps the actor's CLI state in caller-owned storage, so a failed run
    /// can be recovered with the same identity.
    pub fn with_storage(mut self, storage: ActorStorage) -> Self {
        self.storage = Some(storage);
        self
    }

    /// Starts over from empty storage: the CLI's first launch.
    pub fn start_fresh(&self) -> Result<(), CliError> {
        if self.storage.as_ref().is_some_and(ActorStorage::is_resume) {
            return Err(CliError::Storage(
                "cannot clear protected recovery storage".into(),
            ));
        }
        let _idle = self.idle()?;
        let mut store = lock(&self.store);
        match store.root.as_ref() {
            Some(existing) => wipe(existing)?,
            None => store.root = Some(self.open()?),
        }
        store.closed = false;
        Ok(())
    }

    /// Opens the storage unless it is open: a relaunch, or a recovery that
    /// reopens retained storage. An open store is left as it is, even while
    /// an invocation a caller gave up on still runs in it. Recovery first
    /// reconciles the storage on the blocking pool; a caller that gives up
    /// leaves the client busy until that ends.
    pub async fn ensure_open(&self) -> Result<(), CliError> {
        if self.root().is_some() {
            return Ok(());
        }
        let idle = self.idle()?;
        let unopened = lock(&self.store).root.is_none();
        let _idle = match &self.storage {
            Some(ActorStorage::Resume(path)) if unopened => {
                reconciled(Arc::clone(&self.reconcile), path.clone(), idle).await?
            }
            _ => idle,
        };
        let mut store = lock(&self.store);
        if store.root.is_none() {
            store.root = Some(self.open()?);
        }
        store.closed = false;
        Ok(())
    }

    /// The storage root, while the client is open.
    pub fn root(&self) -> Option<PathBuf> {
        lock(&self.store)
            .open_root()
            .map(|root| root.path().to_path_buf())
    }

    /// Waits until no invocation runs: one a caller gave up on is still
    /// bounded by its own deadline, so it ends within that and a grace.
    async fn settled(&self) -> Result<tokio::sync::MutexGuard<'_, ()>, CliError> {
        let bound = self.config.timeout + SETTLE_GRACE;
        tokio::time::timeout(bound, self.serial.lock())
            .await
            .map_err(|_| {
                CliError::Storage(format!("an invocation was still running after {bound:?}"))
            })
    }

    /// Waits until no invocation runs, so nothing of the CLI is alive; one
    /// that outlives its deadline (plus a grace) is a failed stop.
    pub async fn settle(&self) -> Result<(), CliError> {
        self.settled().await.map(drop)
    }

    /// Ends the client once no invocation runs: nothing runs until it is
    /// opened again. A temporary root is released; retained storage stays
    /// on disk for its owner. An invocation that outlives its deadline is a
    /// failed shutdown, and the client stays open.
    pub async fn close(&self) -> Result<(), CliError> {
        let _settled = self.settled().await?;
        let mut store = lock(&self.store);
        if matches!(store.root, Some(Root::Temporary(_))) {
            store.root = None;
        }
        store.closed = true;
        Ok(())
    }

    /// Opens the storage. Retained storage whose runtime never recorded its
    /// stop is refused: [`Self::ensure_open`] reconciles it first.
    fn open(&self) -> Result<Root, CliError> {
        match &self.storage {
            Some(storage) => storage
                .prepare(Platform::Cli)
                .map(Root::Retained)
                .map_err(storage_error),
            None => temporary(),
        }
    }

    /// Runs `call` and returns its `ok` outcome. Invocations of one actor
    /// never overlap: the CLI locks its store for a write.
    pub async fn invoke(&self, call: Call) -> Result<Envelope, CliError> {
        let serial = Arc::clone(&self.serial).lock_owned().await;
        let root = self.root().ok_or_else(|| {
            CliError::Storage("the CLI is not open: never launched, or already closed".into())
        })?;
        let argv = call.argv(&self.config.node);
        let request = ProcessRequest {
            program: self.config.binary.clone(),
            args: argv.clone(),
            env: self.env(&root),
            current_dir: root.join(storage::CLI_HOME),
            stdin: call.stdin().cloned(),
        };
        let process = Arc::clone(&self.process);
        let evidence = self.evidence.clone();
        let timeout = self.config.timeout;
        let reconcile = self.storage.is_some().then(|| Arc::clone(&self.reconcile));
        // The child, its reaping, the runtime marker and the record of the
        // outcome live in their own task: a caller that gives up (a scenario
        // deadline) leaves neither a live child nor a marker that would
        // refuse every later invocation, and the invocation still reaches
        // the transcript.
        let task = tokio::spawn(async move {
            let _serial = serial;
            let active = match reconcile {
                Some(reconcile) => {
                    reconciled(reconcile, root.clone(), ()).await?;
                    Some(ActiveRuntime::begin(&root).map_err(storage_error)?)
                }
                None => None,
            };
            let started = Instant::now();
            let result = process.run(&request, timeout).await;
            let elapsed_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
            let stopped = active.map_or(Ok(()), |active| active.stopped().map_err(storage_error));
            let outcome = evidence.interpret(&call, &argv, elapsed_ms, result);
            Ok::<_, CliError>((outcome, stopped))
        });
        let (outcome, stopped) = task
            .await
            .map_err(|e| CliError::Storage(format!("the CLI invocation task failed: {e}")))??;
        match (stopped, outcome) {
            (Ok(()), outcome) => outcome,
            // What the command did stands, on the node for a node command:
            // the report must not read as if nothing happened.
            (Err(storage), Ok(_)) => Err(CliError::Storage(format!(
                "{storage}; the command itself succeeded, so what it did stands"
            ))),
            (Err(storage), Err(command)) => Err(CliError::Storage(format!(
                "{storage}; the command itself failed: {command}"
            ))),
        }
    }

    fn env(&self, root: &Path) -> BTreeMap<String, String> {
        let mut env = BTreeMap::new();
        let dir = |child| root.join(child).display().to_string();
        env.insert("MCLI_DIR".into(), dir(storage::CLI_STORE));
        env.insert("HOME".into(), dir(storage::CLI_HOME));
        if let Some(path) = &self.config.path_env {
            env.insert("PATH".into(), path.clone());
        }
        env
    }

    /// Transcript lines recorded since the previous call.
    pub fn take_transcript(&self) -> String {
        self.evidence.take_transcript()
    }

    /// The whole transcript and the CLI's standard error, for the run,
    /// once an invocation still running has recorded its outcome. If one
    /// outlives its deadline, what was recorded so far.
    pub async fn diagnostics(&self) -> Vec<(String, String)> {
        let settled = self.settled().await;
        if let Err(error) = &settled {
            tracing::warn!(%error, "collecting the CLI evidence without the running invocation");
        }
        self.evidence.diagnostics()
    }
}

/// Runs `reconcile` on `root` on the blocking pool, where a scan of `/proc`
/// that blocks stalls no runtime thread. The blocking task holds `guard`
/// until it ends, even when the caller gives up, and hands it back.
async fn reconciled<G: Send + 'static>(
    reconcile: Reconcile,
    root: PathBuf,
    guard: G,
) -> Result<G, CliError> {
    let (guard, outcome) = tokio::task::spawn_blocking(move || (guard, reconcile(&root)))
        .await
        .map_err(|e| CliError::Storage(format!("the CLI runtime reconciliation failed: {e}")))?;
    outcome.map(|()| guard)
}

/// Empties the actor's CLI directories, keeping the storage marker. Storage
/// whose previous runtime never recorded its stop is left alone.
fn wipe(root: &Root) -> Result<(), CliError> {
    if let Root::Retained(path) = root {
        storage::ensure_stopped(path).map_err(storage_error)?;
    }
    let root = root.path();
    for child in storage::layout(Platform::Cli) {
        let path = root.join(child);
        match std::fs::remove_dir_all(&path) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(storage_error(e)),
        }
        private_dir(&path)?;
    }
    Ok(())
}

fn temporary() -> Result<Root, CliError> {
    let dir = tempfile::Builder::new()
        .prefix("mortsom-cli-")
        .tempdir()
        .map_err(storage_error)?;
    for child in storage::layout(Platform::Cli) {
        private_dir(&dir.path().join(child))?;
    }
    Ok(Root::Temporary(dir))
}

fn private_dir(path: &Path) -> Result<(), CliError> {
    use std::os::unix::fs::DirBuilderExt;
    std::fs::DirBuilder::new()
        .mode(0o700)
        .create(path)
        .map_err(storage_error)
}

fn storage_error(error: std::io::Error) -> CliError {
    CliError::Storage(error.to_string())
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

#[cfg(test)]
#[path = "client_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "client_reconcile_tests.rs"]
mod reconcile_tests;
