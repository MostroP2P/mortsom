//! One actor's mostro-cli: its private storage, its serialized invocations
//! and their sanitized transcript.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use mortsom_desktop_driver::Platform;
use mortsom_desktop_driver::storage::{ActiveRuntime, ActorStorage};

use mortsom_common::Secret;

use crate::call::{Call, NodeTarget};
use crate::envelope::{Envelope, Status};
use crate::error::CliError;
use crate::process::{CliProcess, ProcessFailure, ProcessOutput, ProcessRequest};
use crate::transcript::{Recorded, Transcript, scrub};

/// How much of the CLI's standard error is kept per actor.
const STDERR_LIMIT: usize = 256 * 1024;
/// The private subdirectories of an actor's CLI storage.
const LAYOUT: [&str; 2] = ["mcli", "home"];

/// What every invocation of one actor shares.
#[derive(Debug, Clone)]
pub struct CliConfig {
    /// The mostro-cli executable.
    pub binary: PathBuf,
    /// The daemon and relays node commands talk to.
    pub node: NodeTarget,
    /// Deadline of one invocation; longer than the CLI's own deadlines.
    pub timeout: Duration,
    /// `PATH` handed to the child, if any.
    pub path_env: Option<String>,
}

/// One actor's command-line client.
pub struct CliClient {
    config: CliConfig,
    process: Arc<dyn CliProcess>,
    storage: Option<ActorStorage>,
    root: Mutex<Option<Root>>,
    transcript: Mutex<Transcript>,
    stderr: Mutex<String>,
    /// Held for the whole life of an invocation, by the task that runs it:
    /// a caller that gives up never releases it while a child still runs.
    serial: Arc<tokio::sync::Mutex<()>>,
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
            root: Mutex::new(None),
            transcript: Mutex::new(Transcript::default()),
            stderr: Mutex::new(String::new()),
            serial: Arc::new(tokio::sync::Mutex::new(())),
        }
    }

    /// Refuses a lifecycle change while an invocation still runs.
    fn idle(&self) -> Result<tokio::sync::MutexGuard<'_, ()>, CliError> {
        self.serial
            .try_lock()
            .map_err(|_| CliError::Storage("an invocation is still running".into()))
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
        let mut root = lock(&self.root);
        match root.as_ref() {
            Some(existing) => wipe(existing),
            None => {
                *root = Some(self.open()?);
                Ok(())
            }
        }
    }

    /// Opens the storage unless it is open: a relaunch, or a recovery that
    /// reopens retained storage.
    pub fn ensure_open(&self) -> Result<(), CliError> {
        let _idle = self.idle()?;
        let mut root = lock(&self.root);
        if root.is_none() {
            *root = Some(self.open()?);
        }
        Ok(())
    }

    /// The storage root, once open.
    pub fn root(&self) -> Option<PathBuf> {
        lock(&self.root)
            .as_ref()
            .map(|root| root.path().to_path_buf())
    }

    /// Releases a temporary root. Retained storage stays for its owner, and
    /// a root an invocation still runs in stays until the process exits.
    pub fn close(&self) {
        let Ok(_idle) = self.idle() else {
            return;
        };
        let mut root = lock(&self.root);
        if matches!(root.as_ref(), Some(Root::Temporary(_))) {
            *root = None;
        }
    }

    fn open(&self) -> Result<Root, CliError> {
        match &self.storage {
            Some(storage) => {
                if let ActorStorage::Resume(path) = storage {
                    crate::runtime::reconcile(path)?;
                }
                storage
                    .prepare(Platform::Cli)
                    .map(Root::Retained)
                    .map_err(storage_error)
            }
            None => temporary(),
        }
    }

    /// Runs `call` and returns its `ok` outcome. Invocations of one actor
    /// never overlap: the CLI locks its store for a write.
    pub async fn invoke(&self, call: Call) -> Result<Envelope, CliError> {
        let serial = Arc::clone(&self.serial).lock_owned().await;
        let root = self
            .root()
            .ok_or_else(|| CliError::Storage("the CLI was never launched".into()))?;
        let argv = call.argv(&self.config.node);
        let request = ProcessRequest {
            program: self.config.binary.clone(),
            args: argv.clone(),
            env: self.env(&root),
            current_dir: root.join("home"),
            stdin: call.stdin().cloned(),
        };
        let process = Arc::clone(&self.process);
        let timeout = self.config.timeout;
        let retained = self.storage.is_some();
        // The child, its reaping and the runtime marker live in their own
        // task: a caller that gives up (a scenario deadline) leaves neither a
        // live child nor a marker that would refuse every later invocation.
        let task = tokio::spawn(async move {
            let _serial = serial;
            let active = match retained {
                true => {
                    crate::runtime::reconcile(&root)?;
                    Some(ActiveRuntime::begin(&root).map_err(storage_error)?)
                }
                false => None,
            };
            let started = Instant::now();
            let result = process.run(&request, timeout).await;
            let elapsed_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
            let stopped = active.map_or(Ok(()), |active| active.stopped().map_err(storage_error));
            Ok::<_, CliError>((result, elapsed_ms, stopped))
        });
        let (result, elapsed_ms, stopped) = task
            .await
            .map_err(|e| CliError::Storage(format!("the CLI invocation task failed: {e}")))??;
        let outcome = self.interpret(&call, &argv, elapsed_ms, result);
        match (stopped, outcome) {
            (Ok(()), outcome) => outcome,
            (Err(storage), Ok(_)) => Err(storage),
            (Err(storage), Err(command)) => Err(CliError::Storage(format!(
                "{storage}; the command itself failed: {command}"
            ))),
        }
    }

    fn interpret(
        &self,
        call: &Call,
        argv: &[String],
        elapsed_ms: u64,
        result: Result<ProcessOutput, ProcessFailure>,
    ) -> Result<Envelope, CliError> {
        let command = call.name();
        let output = match result {
            Ok(output) => output,
            Err(cause) => {
                self.record(
                    &command,
                    argv,
                    elapsed_ms,
                    Recorded::Failure(cause.to_string(), None),
                );
                return Err(CliError::Process { command, cause });
            }
        };
        let secrets: Vec<&Secret> = call.stdin().into_iter().collect();
        self.keep_stderr(&scrub(&output.stderr, &secrets));
        if let Some(error) = without_document(&command, &output) {
            let what = Recorded::Failure(error.to_string(), output.code);
            self.record(&command, argv, elapsed_ms, what);
            return Err(error);
        }
        let envelope = match Envelope::parse(&output.stdout, output.code) {
            Ok(envelope) => envelope,
            Err(detail) => {
                let detail = scrub(&detail, &secrets);
                let what = Recorded::Failure(detail.clone(), output.code);
                self.record(&command, argv, elapsed_ms, what);
                return Err(CliError::Contract { command, detail });
            }
        };
        self.record(
            &command,
            argv,
            elapsed_ms,
            Recorded::Outcome(&envelope, output.code),
        );
        if envelope.command != command {
            let detail = format!("answered as `{}`", envelope.command);
            return Err(CliError::Contract { command, detail });
        }
        outcome(command, envelope)
    }

    fn env(&self, root: &Path) -> BTreeMap<String, String> {
        let mut env = BTreeMap::new();
        env.insert("MCLI_DIR".into(), root.join("mcli").display().to_string());
        env.insert("HOME".into(), root.join("home").display().to_string());
        if let Some(path) = &self.config.path_env {
            env.insert("PATH".into(), path.clone());
        }
        env
    }

    fn record(&self, command: &str, argv: &[String], elapsed_ms: u64, what: Recorded<'_>) {
        lock(&self.transcript).record(command, argv, elapsed_ms, what);
    }

    fn keep_stderr(&self, stderr: &str) {
        if stderr.is_empty() {
            return;
        }
        let mut kept = lock(&self.stderr);
        kept.push_str(stderr);
        if !stderr.ends_with('\n') {
            kept.push('\n');
        }
        if kept.len() > STDERR_LIMIT {
            let cut = kept.len() - STDERR_LIMIT;
            let boundary = (cut..kept.len())
                .find(|i| kept.is_char_boundary(*i))
                .unwrap_or(kept.len());
            kept.drain(..boundary);
        }
    }

    /// Transcript lines recorded since the previous call.
    pub fn take_transcript(&self) -> String {
        lock(&self.transcript).take_unread()
    }

    /// The whole transcript and the CLI's standard error, for the run.
    pub fn diagnostics(&self) -> Vec<(String, String)> {
        let mut files = Vec::new();
        let transcript = lock(&self.transcript);
        if !transcript.is_empty() {
            files.push(("cli-transcript.jsonl".to_owned(), transcript.render()));
        }
        let stderr = lock(&self.stderr);
        if !stderr.is_empty() {
            files.push(("cli-stderr.log".to_owned(), stderr.clone()));
        }
        files
    }
}

/// A child that printed nothing: exit 2 is the argument parser refusing
/// what Mortsom sent (a contract mismatch); any other failure is the CLI
/// crashing, a failure of the system under test.
fn without_document(command: &str, output: &ProcessOutput) -> Option<CliError> {
    if !output.stdout.trim().is_empty() {
        return None;
    }
    match output.code {
        Some(0) | None => None,
        Some(2) => Some(CliError::Contract {
            command: command.to_owned(),
            detail: "the CLI refused its arguments without an outcome document (exit 2)".into(),
        }),
        Some(exit) => Some(CliError::Crashed {
            command: command.to_owned(),
            exit,
        }),
    }
}

fn outcome(command: String, envelope: Envelope) -> Result<Envelope, CliError> {
    let (code, message, reason) = envelope
        .error
        .as_ref()
        .map(|e| (e.code.clone(), e.message.clone(), e.reason.clone()))
        .unwrap_or_default();
    match envelope.status {
        Status::Ok => Ok(envelope),
        Status::Error => Err(CliError::Refused {
            command,
            code,
            message,
            reason,
        }),
        Status::Uncertain => Err(CliError::Uncertain {
            command,
            code,
            message,
        }),
    }
}

/// Empties the actor's CLI directories, keeping the storage marker. Storage
/// whose previous runtime never recorded its stop is left alone.
fn wipe(root: &Root) -> Result<(), CliError> {
    if let Root::Retained(path) = root {
        mortsom_desktop_driver::storage::ensure_stopped(path).map_err(storage_error)?;
    }
    let root = root.path();
    for child in LAYOUT {
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
    for child in LAYOUT {
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
