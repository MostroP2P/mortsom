//! The boundary to the mostro-cli executable: one bounded child process per
//! invocation, with an emptied environment, a private working directory and
//! an optional secret written to standard input. Tests inject a scripted
//! process instead (`testing`).

use std::collections::BTreeMap;
use std::io::ErrorKind;
use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;

use async_trait::async_trait;
use mortsom_common::Secret;
use thiserror::Error;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};
use zeroize::Zeroizing;

/// How much of each output stream is kept; the rest is drained unread so a
/// runaway child can neither block on a full pipe nor exhaust memory.
const OUTPUT_LIMIT: usize = 4 * 1024 * 1024;
/// How long a failed invocation waits for the standard error of a child it
/// already ended: a descendant still holding the pipe cannot hold it up.
const STDERR_GRACE: Duration = Duration::from_secs(1);

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
    async fn run(
        &self,
        request: &ProcessRequest,
        timeout: Duration,
    ) -> Result<ProcessOutput, ProcessFailed>;
}

/// The real child process, killed when its deadline passes.
#[derive(Debug, Default, Clone, Copy)]
pub struct TokioCliProcess;

#[async_trait]
impl CliProcess for TokioCliProcess {
    async fn run(
        &self,
        request: &ProcessRequest,
        timeout: Duration,
    ) -> Result<ProcessOutput, ProcessFailed> {
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
            .kill_on_drop(true);
        let mut child = command
            .spawn()
            .map_err(|e| ProcessFailed::from(ProcessFailure::Spawn(e.to_string())))?;
        let stdout = tokio::spawn(read_bounded(child.stdout.take()));
        let stderr = tokio::spawn(read_bounded(child.stderr.take()));
        let stdin = child.stdin.take();
        let deadline = tokio::time::Instant::now() + timeout;
        let finished = tokio::time::timeout_at(deadline, async {
            if let (Some(mut pipe), Some(secret)) = (stdin, &request.stdin) {
                write_secret(&mut pipe, secret).await?;
                // Dropping the pipe closes it: the child reads end of file.
            }
            child
                .wait()
                .await
                .map_err(|e| ProcessFailure::Io(e.to_string()))
        })
        .await;
        let status = match finished {
            Ok(Ok(status)) => status,
            Ok(Err(failure)) => {
                reap(&mut child).await;
                return Err(failed(failure, stderr).await);
            }
            // Killed and waited for: the child is gone before we say so.
            Err(_) => {
                reap(&mut child).await;
                return Err(failed(ProcessFailure::Timeout(timeout), stderr).await);
            }
        };
        let Some(code) = status.code() else {
            return Err(failed(ProcessFailure::Signaled, stderr).await);
        };
        Ok(ProcessOutput {
            code: Some(code),
            stdout: joined(stdout).await?,
            stderr: joined(stderr).await?,
        })
    }
}

/// `failure`, with what the ended child had written to standard error.
async fn failed(
    failure: ProcessFailure,
    stderr: tokio::task::JoinHandle<std::io::Result<String>>,
) -> ProcessFailed {
    let stderr = match tokio::time::timeout(STDERR_GRACE, joined(stderr)).await {
        Ok(Ok(text)) => text,
        _ => String::new(),
    };
    ProcessFailed { failure, stderr }
}

/// Kills the child and waits for it to be gone.
async fn reap(child: &mut tokio::process::Child) {
    if let Err(error) = child.kill().await {
        tracing::warn!(%error, "cannot kill the CLI process");
    }
}

/// Reads at most [`OUTPUT_LIMIT`] bytes of a stream, draining the rest.
async fn read_bounded(pipe: Option<impl AsyncRead + Unpin>) -> std::io::Result<String> {
    let Some(mut pipe) = pipe else {
        return Ok(String::new());
    };
    let mut kept = Vec::new();
    let mut buffer = [0_u8; 8192];
    loop {
        let read = pipe.read(&mut buffer).await?;
        if read == 0 {
            break;
        }
        let room = OUTPUT_LIMIT.saturating_sub(kept.len());
        kept.extend_from_slice(&buffer[..read.min(room)]);
    }
    Ok(String::from_utf8_lossy(&kept).into_owned())
}

async fn joined(
    task: tokio::task::JoinHandle<std::io::Result<String>>,
) -> Result<String, ProcessFailure> {
    task.await
        .map_err(|e| ProcessFailure::Io(e.to_string()))?
        .map_err(|e| ProcessFailure::Io(e.to_string()))
}

/// Writes the secret and closes the pipe. A child that exits before reading
/// it (an argument refused up front) closes its end first; that is not a
/// failure of ours, and its envelope says why.
async fn write_secret(
    stdin: &mut tokio::process::ChildStdin,
    secret: &Secret,
) -> Result<(), ProcessFailure> {
    let mut line = Zeroizing::new(secret.expose().as_bytes().to_vec());
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
mod tests {
    use super::*;

    fn shell(script: &str, env: &[(&str, &str)], stdin: Option<&str>) -> ProcessRequest {
        ProcessRequest {
            program: PathBuf::from("/bin/sh"),
            args: vec!["-c".into(), script.into()],
            env: env
                .iter()
                .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
                .collect(),
            current_dir: std::env::temp_dir(),
            stdin: stdin.map(Secret::new),
        }
    }

    #[tokio::test]
    async fn the_secret_reaches_standard_input_and_the_exit_code_is_kept() {
        let request = shell("cat; echo oops >&2; exit 3", &[], Some("lnbcrt1secret"));

        let output = TokioCliProcess
            .run(&request, Duration::from_secs(10))
            .await
            .unwrap();

        assert_eq!(output.code, Some(3));
        assert_eq!(output.stdout, "lnbcrt1secret\n");
        assert_eq!(output.stderr.trim(), "oops");
    }

    #[tokio::test]
    async fn the_child_inherits_no_environment() {
        let request = shell(
            "echo \"${HOME:-unset} ${MCLI_DIR:-unset}\"",
            &[("MCLI_DIR", "/private")],
            None,
        );

        let output = TokioCliProcess
            .run(&request, Duration::from_secs(10))
            .await
            .unwrap();

        assert_eq!(output.stdout.trim(), "unset /private");
    }

    #[tokio::test]
    async fn without_a_secret_standard_input_is_already_at_its_end() {
        let request = shell("cat; echo done", &[], None);

        let output = TokioCliProcess
            .run(&request, Duration::from_secs(10))
            .await
            .unwrap();

        assert_eq!(output.stdout, "done\n");
    }

    #[tokio::test]
    async fn a_child_that_never_reads_its_secret_is_not_a_pipe_failure() {
        let request = shell("exec 0<&-; echo refused; exit 2", &[], Some("x"));

        let output = TokioCliProcess
            .run(&request, Duration::from_secs(10))
            .await
            .unwrap();

        assert_eq!(output.code, Some(2));
    }

    #[tokio::test]
    async fn a_child_past_its_deadline_is_a_timeout() {
        let request = shell("while :; do :; done", &[], None);

        let failure = TokioCliProcess
            .run(&request, Duration::from_millis(200))
            .await
            .unwrap_err();

        assert_eq!(
            failure.failure,
            ProcessFailure::Timeout(Duration::from_millis(200))
        );
    }

    #[tokio::test]
    async fn a_child_ended_by_a_signal_is_no_output() {
        let request = shell("kill -9 $$", &[], None);

        let failure = TokioCliProcess
            .run(&request, Duration::from_secs(10))
            .await
            .unwrap_err();

        assert_eq!(failure.failure, ProcessFailure::Signaled);
    }

    #[tokio::test]
    async fn a_timed_out_child_is_gone_when_the_timeout_is_reported() {
        let dir = tempfile::tempdir().unwrap();
        let pid_file = dir.path().join("pid");
        let script = format!("echo $$ > {}; while :; do :; done", pid_file.display());
        let request = shell(&script, &[], None);

        let failure = TokioCliProcess
            .run(&request, Duration::from_millis(300))
            .await
            .unwrap_err();

        assert_eq!(
            failure.failure,
            ProcessFailure::Timeout(Duration::from_millis(300))
        );
        let pid = std::fs::read_to_string(&pid_file).unwrap();
        let proc_dir = std::path::Path::new("/proc").join(pid.trim());
        assert!(!proc_dir.exists(), "the timed-out child is still alive");
    }

    #[tokio::test]
    async fn a_missing_executable_is_a_spawn_failure() {
        let mut request = shell("true", &[], None);
        request.program = PathBuf::from("/nonexistent/mostro-cli");

        let failure = TokioCliProcess
            .run(&request, Duration::from_secs(1))
            .await
            .unwrap_err();

        assert!(
            matches!(failure.failure, ProcessFailure::Spawn(_)),
            "{failure:?}"
        );
    }

    /// A child that hangs or is killed may have said why on standard error:
    /// the failure carries it, so the run's evidence keeps it.
    #[tokio::test]
    async fn a_failed_child_keeps_what_it_wrote_to_standard_error() {
        let hung = shell("echo stuck >&2; exec sleep 30", &[], None);
        let killed = shell("echo bye >&2; kill -9 $$", &[], None);

        let timed_out = TokioCliProcess
            .run(&hung, Duration::from_millis(300))
            .await
            .unwrap_err();
        let signaled = TokioCliProcess
            .run(&killed, Duration::from_secs(10))
            .await
            .unwrap_err();

        assert!(matches!(timed_out.failure, ProcessFailure::Timeout(_)));
        assert_eq!(timed_out.stderr.trim(), "stuck");
        assert_eq!(signaled.failure, ProcessFailure::Signaled);
        assert_eq!(signaled.stderr.trim(), "bye");
    }
}
