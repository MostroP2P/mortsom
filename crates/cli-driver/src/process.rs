//! The boundary to the mostro-cli executable: one bounded child process per
//! invocation, leading its own process group, with an emptied environment,
//! a private working directory and an optional secret written to standard
//! input. Tests inject a scripted process instead (`testing`).

use std::collections::BTreeMap;
use std::io::ErrorKind;
use std::path::PathBuf;
use std::process::{ExitStatus, Stdio};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use async_trait::async_trait;
use mortsom_common::Secret;
use rustix::io::Errno;
use rustix::process::{
    Pid, Signal, WaitId, WaitIdOptions, WaitIdStatus, kill_process_group, waitid,
};
use thiserror::Error;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};
use tokio::time::{Instant, MissedTickBehavior};
use zeroize::Zeroizing;

/// How much of each output stream is kept; the rest is drained unread so a
/// runaway child can neither block on a full pipe nor exhaust memory.
const OUTPUT_LIMIT: usize = 4 * 1024 * 1024;
/// How long a failed invocation waits for the standard error of a child it
/// already ended: a holder that left its group cannot hold it up.
const STDERR_GRACE: Duration = Duration::from_secs(1);
/// How often a running child is checked for an exit, which is observed
/// without reaping the child.
const EXIT_POLL: Duration = Duration::from_millis(10);

/// One invocation of the executable.
#[derive(Debug, Clone)]
pub struct ProcessRequest {
    /// Executable path.
    pub program: PathBuf,
    /// Arguments. Never a secret: secrets travel on standard input.
    pub args: Vec<String>,
    /// The complete environment of the child: nothing is inherited.
    pub env: BTreeMap<String, String>,
    /// Working directory, so no stray `.env` of the caller's is read.
    pub current_dir: PathBuf,
    /// Written to standard input followed by a newline, then closed. `None`
    /// connects standard input to nothing, so the child reads end of file.
    pub stdin: Option<Secret>,
}

/// What a finished child left behind.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessOutput {
    /// Exit code; `None` when a signal ended the child.
    pub code: Option<i32>,
    /// Standard output.
    pub stdout: String,
    /// Standard error.
    pub stderr: String,
}

/// Why no output was obtained.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ProcessFailure {
    /// The executable could not be started.
    #[error("cannot start the CLI: {0}")]
    Spawn(String),
    /// The child did not finish in time and was killed.
    #[error("the CLI did not finish within {0:?}")]
    Timeout(Duration),
    /// The child's pipes failed.
    #[error("cannot talk to the CLI process: {0}")]
    Io(String),
    /// A signal ended the child: it was killed from outside, never exited.
    #[error("the CLI was ended by a signal")]
    Signaled,
}

/// A run that produced no output, with the standard error the child wrote
/// before it ended (a timed-out or killed child may have said why).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessFailed {
    /// What went wrong.
    pub failure: ProcessFailure,
    /// The child's standard error, bounded and not yet sanitized.
    pub stderr: String,
}

impl From<ProcessFailure> for ProcessFailed {
    fn from(failure: ProcessFailure) -> Self {
        Self {
            failure,
            stderr: String::new(),
        }
    }
}

impl std::fmt::Display for ProcessFailed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.failure.fmt(f)
    }
}

/// Runs the executable once per request.
#[async_trait]
pub trait CliProcess: Send + Sync {
    /// Runs `request` to completion within `timeout`. A non-zero exit is an
    /// output, never a failure: the envelope on stdout says what happened.
    /// A child that outlives `timeout`, or fails before it exits, is killed,
    /// and its standard error is then awaited for at most one more second
    /// (the tail of a hung CLI's standard error is the best evidence of why).
    async fn run(
        &self,
        request: &ProcessRequest,
        timeout: Duration,
    ) -> Result<ProcessOutput, ProcessFailed>;
}

/// The real child process. Whether it exits, fails or outlives its deadline,
/// every descendant still in its process group is killed before it is
/// reaped.
#[derive(Debug, Default, Clone, Copy)]
pub struct TokioCliProcess;

#[async_trait]
impl CliProcess for TokioCliProcess {
    async fn run(
        &self,
        request: &ProcessRequest,
        timeout: Duration,
    ) -> Result<ProcessOutput, ProcessFailed> {
        let mut leader = Leader::spawn(&mut command(request))
            .map_err(|e| ProcessFailed::from(ProcessFailure::Spawn(e.to_string())))?;
        let mut stdout = Reader::spawn(leader.child.stdout.take());
        let mut stderr = Reader::spawn(leader.child.stderr.take());
        let deadline = Instant::now() + timeout;
        let exited = tokio::time::timeout_at(deadline, leader.exited(request.stdin.as_ref()))
            .await
            .unwrap_or(Err(ProcessFailure::Timeout(timeout)));
        // Exited, failed or late, the leader is still unreaped, so the group
        // id is still its own: for a one-shot CLI, any member left is a leak.
        leader.kill_group();
        if let Err(failure) = exited {
            // Killed and reaped: the child is gone before we say so.
            return Err(leader.end(failure, &mut stderr).await);
        }
        // The group is never signalled again, so the leader can go now.
        let status = match leader.reap().await {
            Ok(status) => status,
            Err(failure) => return Err(failed(failure, &mut stderr).await),
        };
        // A holder that left the group can still keep the output open: the
        // deadline bounds reading it, and a leader a signal ended, which gave
        // no answer, gets only the grace for its standard error.
        let until = match status.code() {
            Some(_) => deadline,
            None => deadline.min(Instant::now() + STDERR_GRACE),
        };
        let collected = Collected::within(until, &mut stdout, &mut stderr).await;
        outcome(status, collected, timeout)
    }
}

/// The command for `request`: an emptied environment, piped output, and a
/// process group of its own.
fn command(request: &ProcessRequest) -> tokio::process::Command {
    let mut command = tokio::process::Command::new(&request.program);
    command
        .args(&request.args)
        .env_clear()
        .envs(&request.env)
        .current_dir(&request.current_dir)
        .stdin(if request.stdin.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0)
        .kill_on_drop(true);
    command
}

/// The child, leader of its own process group: ending the group ends every
/// descendant that stayed in it, so none outlives the invocation holding
/// the actor's directory.
///
/// The group id is the leader's pid, which stays the leader's until the
/// leader is reaped: an exited leader is a zombie that is still a member of
/// its group. So its exit is observed without reaping it, the group is
/// signalled only while it is unreaped, and the reap comes last. The app
/// processes of the UI driver are ended the same way.
struct Leader {
    child: tokio::process::Child,
    pid: Pid,
}

impl Leader {
    fn spawn(command: &mut tokio::process::Command) -> std::io::Result<Self> {
        let child = command.spawn()?;
        let pid = child
            .id()
            .and_then(|id| i32::try_from(id).ok())
            .and_then(Pid::from_raw)
            .ok_or_else(|| std::io::Error::other("the started CLI has no process id"))?;
        Ok(Self { child, pid })
    }

    /// Hands the child its secret, if any, and waits for it to exit,
    /// leaving it unreaped.
    async fn exited(&mut self, secret: Option<&Secret>) -> Result<(), ProcessFailure> {
        if let (Some(mut pipe), Some(secret)) = (self.child.stdin.take(), secret) {
            write_secret(&mut pipe, secret).await?;
            // Dropping the pipe closes it: the child reads end of file.
        }
        let mut poll = tokio::time::interval(EXIT_POLL);
        poll.set_missed_tick_behavior(MissedTickBehavior::Delay);
        loop {
            poll.tick().await;
            match self.status() {
                Ok(Some(_)) => return Ok(()),
                Ok(None) => {}
                Err(error) => return Err(ProcessFailure::Io(error.to_string())),
            }
        }
    }

    /// The leader's exit, if it has exited, observed without reaping it.
    /// ECHILD says the leader is no longer ours to wait for, nor its pid.
    fn status(&self) -> rustix::io::Result<Option<WaitIdStatus>> {
        let options = WaitIdOptions::EXITED | WaitIdOptions::NOHANG | WaitIdOptions::NOWAIT;
        waitid(WaitId::Pid(self.pid), options)
    }

    /// Sends SIGKILL to every member of the group, provided the leader is
    /// still ours and unreaped, so the id cannot name an unrelated group.
    fn kill_group(&self) {
        if let Err(error) = self.status() {
            tracing::warn!(%error, "the CLI process is no longer ours; its group is left alone");
            return;
        }
        match kill_process_group(self.pid, Signal::KILL) {
            Ok(()) => {}
            // The unreaped leader belongs to its group unless it never got
            // one of its own or moved itself to another.
            Err(Errno::SRCH) => tracing::warn!("the CLI process group is gone before its reap"),
            Err(error) => tracing::warn!(%error, "cannot kill the CLI process group"),
        }
    }

    /// Ends a leader that did not exit in time or that we failed to talk to,
    /// once its group is ended: the leader itself too, in case it left the
    /// group. Then waits briefly for its standard error, reaps it and
    /// returns `failure` with that text.
    async fn end(&mut self, failure: ProcessFailure, stderr: &mut Reader) -> ProcessFailed {
        if self.status().is_ok()
            && let Err(error) = self.child.start_kill()
        {
            tracing::warn!(%error, "cannot kill the CLI process");
        }
        let failed = failed(failure, stderr).await;
        if let Err(error) = self.reap().await {
            tracing::warn!(%error, "cannot reap the CLI process");
        }
        failed
    }

    /// Reaps the leader, which has exited or been killed: its status was
    /// fixed when it ended.
    async fn reap(&mut self) -> Result<ExitStatus, ProcessFailure> {
        self.child
            .wait()
            .await
            .map_err(|e| ProcessFailure::Io(e.to_string()))
    }
}

impl Drop for Leader {
    fn drop(&mut self) {
        // Still unreaped: the run was abandoned, and `kill_on_drop` alone
        // would leave the descendants running.
        if self.child.id().is_some() {
            self.kill_group();
        }
    }
}

/// `failure`, with what the ended child had written to standard error.
async fn failed(failure: ProcessFailure, stderr: &mut Reader) -> ProcessFailed {
    let stderr = match stderr.finish_within(STDERR_GRACE).await {
        Some(text) => text,
        // A holder that left the group still has the pipe: keep what was read.
        None => stderr.so_far(),
    };
    ProcessFailed { failure, stderr }
}

/// What became of the output of a leader that exited.
#[derive(Debug, PartialEq, Eq)]
enum Collected {
    /// Both streams ended.
    Whole { stdout: String, stderr: String },
    /// A stream could not be read; standard error as far as it was read.
    Failed {
        failure: ProcessFailure,
        stderr: String,
    },
    /// Something still held the output at the deadline; standard error so
    /// far.
    Late { stderr: String },
}

impl Collected {
    /// Reads both streams to their end, or until `deadline`.
    async fn within(deadline: Instant, stdout: &mut Reader, stderr: &mut Reader) -> Self {
        let both = tokio::time::timeout_at(deadline, async {
            tokio::try_join!(stdout.finish(), stderr.finish())
        })
        .await;
        match both {
            Ok(Ok((stdout, stderr))) => Self::Whole { stdout, stderr },
            Ok(Err(failure)) => Self::Failed {
                failure,
                stderr: stderr.so_far(),
            },
            Err(_) => Self::Late {
                stderr: stderr.so_far(),
            },
        }
    }

    /// Standard error, whole or as far as it was read.
    fn into_stderr(self) -> String {
        match self {
            Self::Whole { stderr, .. } | Self::Failed { stderr, .. } | Self::Late { stderr } => {
                stderr
            }
        }
    }
}

/// The run's result: an output when the leader exited with a code and its
/// output ended in time. A leader a signal ended gave no answer, whatever
/// became of its output.
fn outcome(
    status: ExitStatus,
    collected: Collected,
    timeout: Duration,
) -> Result<ProcessOutput, ProcessFailed> {
    let Some(code) = status.code() else {
        return Err(ProcessFailed {
            failure: ProcessFailure::Signaled,
            stderr: collected.into_stderr(),
        });
    };
    match collected {
        Collected::Whole { stdout, stderr } => Ok(ProcessOutput {
            code: Some(code),
            stdout,
            stderr,
        }),
        Collected::Failed { failure, stderr } => Err(ProcessFailed { failure, stderr }),
        Collected::Late { stderr } => Err(ProcessFailed {
            failure: ProcessFailure::Timeout(timeout),
            stderr,
        }),
    }
}

/// One output stream, read in its own task into a bounded buffer the caller
/// can see before the stream ends. Dropping it stops the reading, so a
/// descendant holding the pipe holds up nothing.
struct Reader {
    task: tokio::task::JoinHandle<std::io::Result<()>>,
    kept: Arc<Mutex<Vec<u8>>>,
}

impl Reader {
    fn spawn(pipe: Option<impl AsyncRead + Unpin + Send + 'static>) -> Self {
        let kept = Arc::new(Mutex::new(Vec::new()));
        let task = tokio::spawn(read_bounded(pipe, Arc::clone(&kept)));
        Self { task, kept }
    }

    /// The whole stream, once it ends.
    async fn finish(&mut self) -> Result<String, ProcessFailure> {
        (&mut self.task)
            .await
            .map_err(|e| ProcessFailure::Io(e.to_string()))?
            .map_err(|e| ProcessFailure::Io(e.to_string()))?;
        Ok(self.so_far())
    }

    /// The whole stream if it ends within `grace`; `None` while something
    /// still holds it open. A stream that fails yields what was read, since
    /// the caller is already reporting the failure that ended the child.
    async fn finish_within(&mut self, grace: Duration) -> Option<String> {
        match tokio::time::timeout(grace, self.finish()).await {
            Ok(Ok(text)) => Some(text),
            Ok(Err(_)) => Some(self.so_far()),
            Err(_) => None,
        }
    }

    /// What was read so far.
    fn so_far(&self) -> String {
        let kept = self.kept.lock().unwrap_or_else(PoisonError::into_inner);
        String::from_utf8_lossy(&kept).into_owned()
    }
}

impl Drop for Reader {
    fn drop(&mut self) {
        self.task.abort();
    }
}

/// Reads at most [`OUTPUT_LIMIT`] bytes of a stream into `kept`, draining
/// the rest.
async fn read_bounded(
    pipe: Option<impl AsyncRead + Unpin>,
    kept: Arc<Mutex<Vec<u8>>>,
) -> std::io::Result<()> {
    let Some(mut pipe) = pipe else {
        return Ok(());
    };
    let mut buffer = [0_u8; 8192];
    loop {
        let read = pipe.read(&mut buffer).await?;
        if read == 0 {
            return Ok(());
        }
        let mut kept = kept.lock().unwrap_or_else(PoisonError::into_inner);
        let room = OUTPUT_LIMIT.saturating_sub(kept.len());
        kept.extend_from_slice(&buffer[..read.min(room)]);
    }
}

/// Writes the secret and closes the pipe. A child that exits before reading
/// it (an argument refused up front) closes its end first; that is not a
/// failure of ours, and its envelope says why.
async fn write_secret(
    stdin: &mut tokio::process::ChildStdin,
    secret: &Secret,
) -> Result<(), ProcessFailure> {
    // Sized for the newline up front: growing the buffer would free a copy
    // of the secret without wiping it.
    let bytes = secret.expose().as_bytes();
    let mut line = Zeroizing::new(Vec::with_capacity(bytes.len() + 1));
    line.extend_from_slice(bytes);
    line.push(b'\n');
    match stdin.write_all(&line).await {
        Ok(()) => stdin
            .shutdown()
            .await
            .or_else(ignore_broken_pipe)
            .map_err(|e| ProcessFailure::Io(e.to_string())),
        Err(e) => ignore_broken_pipe(e).map_err(|e| ProcessFailure::Io(e.to_string())),
    }
}

fn ignore_broken_pipe(error: std::io::Error) -> std::io::Result<()> {
    if error.kind() == ErrorKind::BrokenPipe {
        Ok(())
    } else {
        Err(error)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
#[path = "process_tests.rs"]
mod tests;
