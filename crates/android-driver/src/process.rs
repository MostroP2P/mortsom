//! Process boundary for Android tooling (`adb`, `emulator`, `avdmanager`,
//! `appium`). The real runner uses `tokio::process`; tests inject a fake.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use async_trait::async_trait;
use thiserror::Error;
use tokio::sync::Mutex;

#[cfg(target_os = "linux")]
mod group;

/// A command to run: program, arguments and extra environment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Command {
    /// Executable path.
    pub program: PathBuf,
    /// Arguments.
    pub args: Vec<String>,
    /// Extra environment variables.
    pub env: BTreeMap<String, String>,
}

impl Command {
    /// Builds a command.
    pub fn new(program: impl Into<PathBuf>, args: &[&str]) -> Self {
        Self {
            program: program.into(),
            args: args.iter().map(|a| (*a).to_owned()).collect(),
            env: BTreeMap::new(),
        }
    }

    /// Adds an environment variable.
    pub fn env(mut self, key: &str, value: &str) -> Self {
        self.env.insert(key.to_owned(), value.to_owned());
        self
    }

    /// Human-readable rendering for logs (`program arg1 arg2`).
    pub fn display(&self) -> String {
        let mut out = self.program.display().to_string();
        for arg in &self.args {
            out.push(' ');
            out.push_str(arg);
        }
        out
    }
}

/// Captured output of a finished process.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Output {
    /// Exit success.
    pub success: bool,
    /// Standard output.
    pub stdout: String,
    /// Standard error.
    pub stderr: String,
}

/// Process failures.
#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum ProcessError {
    /// The program could not be started.
    #[error("cannot start `{command}`: {message}")]
    Spawn {
        /// Rendered command.
        command: String,
        /// OS error text.
        message: String,
    },
    /// The command did not finish in time and was killed.
    #[error("`{command}` timed out after {timeout:?}")]
    Timeout {
        /// Rendered command.
        command: String,
        /// Deadline used.
        timeout: Duration,
    },
    /// The command exited unsuccessfully.
    #[error("`{command}` failed: {stderr}")]
    Failed {
        /// Rendered command.
        command: String,
        /// Trimmed stderr (or stdout when stderr is empty).
        stderr: String,
    },
}

/// A long-lived child process (emulator, Appium server).
#[async_trait]
pub trait ChildProcess: Send + Sync {
    /// Kills the process; a process that already exited is not an error.
    async fn kill(&self) -> Result<(), ProcessError>;
    /// Whether the process is still running.
    async fn is_alive(&self) -> bool;
}

/// Runs and spawns processes.
#[async_trait]
pub trait CommandRunner: Send + Sync {
    /// Runs to completion with a deadline, capturing output. Never fails on
    /// a non-zero exit; callers decide via [`Output::success`].
    async fn run(&self, command: &Command, timeout: Duration) -> Result<Output, ProcessError>;
    /// Starts a long-lived process, discarding its output to `log_file` if given.
    async fn spawn(
        &self,
        command: &Command,
        log_file: Option<&Path>,
    ) -> Result<Box<dyn ChildProcess>, ProcessError>;
}

/// Runs `command` and treats a non-zero exit as [`ProcessError::Failed`].
pub async fn run_ok(
    runner: &dyn CommandRunner,
    command: &Command,
    timeout: Duration,
) -> Result<Output, ProcessError> {
    let output = runner.run(command, timeout).await?;
    if output.success {
        Ok(output)
    } else {
        let stderr = if output.stderr.trim().is_empty() {
            output.stdout.trim().to_owned()
        } else {
            output.stderr.trim().to_owned()
        };
        Err(ProcessError::Failed {
            command: command.display(),
            stderr,
        })
    }
}

/// The real process runner.
#[derive(Debug, Default, Clone, Copy)]
pub struct TokioRunner;

/// A real child handle. The child is started in its own process group so
/// that helpers it forks (the emulator launcher re-executes qemu) die with it.
pub struct TokioChild {
    child: Mutex<tokio::process::Child>,
    pid: Option<u32>,
    stopped: std::sync::atomic::AtomicBool,
}

#[async_trait]
impl ChildProcess for TokioChild {
    async fn kill(&self) -> Result<(), ProcessError> {
        let mut child = self.child.lock().await;
        if self.stopped.load(std::sync::atomic::Ordering::Acquire) {
            return Ok(());
        }
        #[cfg(not(target_os = "linux"))]
        if child.try_wait().map_err(process_cleanup_error)?.is_some() {
            self.stopped
                .store(true, std::sync::atomic::Ordering::Release);
            return Ok(());
        }
        #[cfg(target_os = "linux")]
        self.stop_owned_group().await?;
        #[cfg(all(unix, not(target_os = "linux")))]
        if let Some(pid) = self.pid {
            let status = tokio::process::Command::new("kill")
                .args(["-9", "--", &format!("-{pid}")])
                .status()
                .await
                .map_err(process_cleanup_error)?;
            if !status.success() {
                return Err(process_cleanup_error("process group kill failed"));
            }
        }
        #[cfg(not(unix))]
        child.start_kill().map_err(process_cleanup_error)?;
        child.wait().await.map_err(process_cleanup_error)?;
        self.stopped
            .store(true, std::sync::atomic::Ordering::Release);
        Ok(())
    }

    async fn is_alive(&self) -> bool {
        #[cfg(target_os = "linux")]
        let _child = self.child.lock().await;
        if self.stopped.load(std::sync::atomic::Ordering::Acquire) {
            return false;
        }
        #[cfg(target_os = "linux")]
        return match self.group_status() {
            Ok(status) => status.is_none(),
            Err(cause) => {
                tracing::warn!(%cause, "cannot inspect owned child");
                false
            }
        };
        #[cfg(not(target_os = "linux"))]
        matches!(self.child.lock().await.try_wait(), Ok(None))
    }
}

fn process_cleanup_error(cause: impl std::fmt::Display) -> ProcessError {
    ProcessError::Spawn {
        command: "owned process cleanup".into(),
        message: cause.to_string(),
    }
}

#[cfg(target_os = "linux")]
impl TokioChild {
    async fn stop_owned_group(&self) -> Result<(), ProcessError> {
        self.kill_owned_group()?;
        let pid = self
            .pid
            .ok_or_else(|| process_cleanup_error("missing child identity"))?;
        tokio::time::timeout(Duration::from_secs(5), async {
            let mut tick = tokio::time::interval(Duration::from_millis(25));
            loop {
                tick.tick().await;
                if !group::has_running_members(pid).map_err(process_cleanup_error)? {
                    return Ok(());
                }
                self.kill_owned_group()?;
            }
        })
        .await
        .map_err(|_| ProcessError::Timeout {
            command: "owned process group shutdown".into(),
            timeout: Duration::from_secs(5),
        })?
    }

    fn group_pid(&self) -> Result<rustix::process::Pid, ProcessError> {
        self.pid
            .and_then(|pid| i32::try_from(pid).ok())
            .and_then(rustix::process::Pid::from_raw)
            .ok_or_else(|| process_cleanup_error("missing child identity"))
    }

    fn group_status(&self) -> Result<Option<rustix::process::WaitIdStatus>, ProcessError> {
        use rustix::process::{WaitId, WaitIdOptions, waitid};
        // NOWAIT leaves an exited leader unreaped. Its PID/group ID cannot be
        // reused while we retain this child, even after is_alive reports false.
        waitid(
            WaitId::Pid(self.group_pid()?),
            WaitIdOptions::EXITED | WaitIdOptions::NOHANG | WaitIdOptions::NOWAIT,
        )
        .map_err(process_cleanup_error)
    }

    fn kill_owned_group(&self) -> Result<(), ProcessError> {
        self.group_status()?; // ECHILD refuses an identity we no longer own.
        match rustix::process::kill_process_group(self.group_pid()?, rustix::process::Signal::KILL)
        {
            Ok(()) | Err(rustix::io::Errno::SRCH) => Ok(()),
            Err(cause) => Err(process_cleanup_error(cause)),
        }
    }
}

impl Drop for TokioChild {
    fn drop(&mut self) {
        #[cfg(target_os = "linux")]
        if !self.stopped.load(std::sync::atomic::Ordering::Acquire)
            && let Err(cause) = self.kill_owned_group()
        {
            tracing::warn!(%cause, "cannot stop dropped owned process group");
        }
    }
}

#[async_trait]
impl CommandRunner for TokioRunner {
    async fn run(&self, command: &Command, timeout: Duration) -> Result<Output, ProcessError> {
        // stdin is closed so a tool that prompts (avdmanager asks about
        // custom hardware profiles) fails fast instead of hanging.
        let future = tokio::process::Command::new(&command.program)
            .args(&command.args)
            .envs(&command.env)
            .stdin(Stdio::null())
            .kill_on_drop(true)
            .output();
        let output = tokio::time::timeout(timeout, future)
            .await
            .map_err(|_| ProcessError::Timeout {
                command: command.display(),
                timeout,
            })?
            .map_err(|e| ProcessError::Spawn {
                command: command.display(),
                message: e.to_string(),
            })?;
        Ok(Output {
            success: output.status.success(),
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        })
    }

    async fn spawn(
        &self,
        command: &Command,
        log_file: Option<&Path>,
    ) -> Result<Box<dyn ChildProcess>, ProcessError> {
        let (stdout, stderr) = match log_file {
            Some(path) => {
                let file = std::fs::File::create(path).map_err(|e| ProcessError::Spawn {
                    command: command.display(),
                    message: format!("cannot create log {}: {e}", path.display()),
                })?;
                let err = file.try_clone().map_err(|e| ProcessError::Spawn {
                    command: command.display(),
                    message: e.to_string(),
                })?;
                (Stdio::from(file), Stdio::from(err))
            }
            None => (Stdio::null(), Stdio::null()),
        };
        let mut builder = tokio::process::Command::new(&command.program);
        builder
            .args(&command.args)
            .envs(&command.env)
            .stdin(Stdio::null())
            .stdout(stdout)
            .stderr(stderr)
            .kill_on_drop(true);
        #[cfg(unix)]
        builder.process_group(0);
        let child = builder.spawn().map_err(|e| ProcessError::Spawn {
            command: command.display(),
            message: e.to_string(),
        })?;
        let pid = child.id();
        Ok(Box::new(TokioChild {
            child: Mutex::new(child),
            pid,
            stopped: std::sync::atomic::AtomicBool::new(false),
        }))
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn command_renders_for_logs() {
        let command = Command::new("/bin/adb", &["-s", "emulator-5554", "shell"]).env("A", "1");
        assert_eq!(command.display(), "/bin/adb -s emulator-5554 shell");
        assert_eq!(command.env.get("A").map(String::as_str), Some("1"));
    }

    #[tokio::test]
    async fn real_runner_captures_output_and_maps_failures() {
        let runner = TokioRunner;
        let ok = run_ok(
            &runner,
            &Command::new("sh", &["-c", "echo hi"]),
            Duration::from_secs(5),
        )
        .await
        .unwrap();
        assert_eq!(ok.stdout.trim(), "hi");

        let failed = run_ok(
            &runner,
            &Command::new("sh", &["-c", "echo bad >&2; exit 3"]),
            Duration::from_secs(5),
        )
        .await
        .unwrap_err();
        assert!(matches!(failed, ProcessError::Failed { ref stderr, .. } if stderr == "bad"));

        let missing = runner
            .run(
                &Command::new("/nonexistent/prog", &[]),
                Duration::from_secs(1),
            )
            .await
            .unwrap_err();
        assert!(matches!(missing, ProcessError::Spawn { .. }));

        let slow = runner
            .run(
                &Command::new("sh", &["-c", "sleep 5"]),
                Duration::from_millis(100),
            )
            .await
            .unwrap_err();
        assert!(matches!(slow, ProcessError::Timeout { .. }));
    }

    #[tokio::test]
    #[cfg(target_os = "linux")]
    async fn killing_an_exited_leader_stops_its_surviving_descendant() {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("descendant.pid");
        let child = TokioRunner
            .spawn(
                &Command::new("sh", &["-c", "sleep 2 & echo $!; exit 0"]),
                Some(&log),
            )
            .await
            .unwrap();
        tokio::time::timeout(Duration::from_secs(1), async {
            while child.is_alive().await {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        let pid: u32 = std::fs::read_to_string(log)
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        child.kill().await.unwrap();
        let state = std::fs::read_to_string(format!("/proc/{pid}/stat"));
        assert!(
            state.is_err() || state.unwrap().split(") ").nth(1).unwrap().starts_with('Z'),
            "successful group cleanup must already have stopped descendants"
        );
        child.kill().await.unwrap();
    }

    #[tokio::test]
    async fn killing_one_spawned_process_group_leaves_the_sibling_alive() {
        let runner = TokioRunner;
        let alice = runner
            .spawn(&Command::new("sh", &["-c", "sleep 30"]), None)
            .await
            .unwrap();
        let bob = runner
            .spawn(&Command::new("sh", &["-c", "sleep 30"]), None)
            .await
            .unwrap();

        alice.kill().await.unwrap();

        assert!(!alice.is_alive().await);
        assert!(
            bob.is_alive().await,
            "sibling in another process group survives"
        );
        bob.kill().await.unwrap();
    }

    #[tokio::test]
    async fn spawn_writes_log_and_kill_is_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("child.log");
        let child = TokioRunner
            .spawn(
                &Command::new("sh", &["-c", "echo started; sleep 30"]),
                Some(&log),
            )
            .await
            .unwrap();
        assert!(child.is_alive().await);
        // Bounded wait for the child to write its first line.
        for _ in 0..50 {
            if std::fs::read_to_string(&log)
                .unwrap_or_default()
                .contains("started")
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }

        child.kill().await.unwrap();
        child.kill().await.unwrap();
        assert!(!child.is_alive().await);
        assert!(std::fs::read_to_string(&log).unwrap().contains("started"));
    }
}

/// Scripted runner shared by unit tests across the workspace (enabled by
/// the `testing` feature or `cfg(test)`).
#[cfg(any(test, feature = "testing"))]
#[allow(clippy::unwrap_used, clippy::expect_used)]
pub mod tests_support {
    use std::collections::BTreeMap;
    use std::sync::{Arc, Mutex};

    use super::*;

    /// Records calls and answers with scripted outputs keyed by a prefix
    /// of `program-name args...`.
    #[derive(Default)]
    pub struct FakeRunner {
        scripts: Mutex<BTreeMap<String, Output>>,
        timeouts: Mutex<Vec<String>>,
        calls: Mutex<Vec<String>>,
        spawned: Mutex<Vec<String>>,
        children: Mutex<Vec<Arc<Mutex<bool>>>>,
    }

    /// A fake child that is alive until killed.
    pub struct FakeChild {
        alive: Arc<Mutex<bool>>,
    }

    impl FakeRunner {
        /// Scripts a successful output for commands starting with `prefix`.
        pub fn script(&self, prefix: &str, stdout: &str) {
            self.scripts.lock().unwrap().insert(
                prefix.to_owned(),
                Output {
                    success: true,
                    stdout: stdout.to_owned(),
                    stderr: String::new(),
                },
            );
        }

        /// Scripts a failing output.
        pub fn script_failure(&self, prefix: &str, stderr: &str) {
            self.scripts.lock().unwrap().insert(
                prefix.to_owned(),
                Output {
                    success: false,
                    stdout: String::new(),
                    stderr: stderr.to_owned(),
                },
            );
        }

        /// Scripts a hang: commands starting with `prefix` time out.
        pub fn script_timeout(&self, prefix: &str) {
            self.timeouts.lock().unwrap().push(prefix.to_owned());
        }

        /// Every command run so far (`program-name args`).
        pub fn calls(&self) -> Vec<String> {
            self.calls.lock().unwrap().clone()
        }

        /// Liveness of every child spawned so far, in spawn order. A child
        /// left alive after a failed startup is a leaked process.
        pub fn children_alive(&self) -> Vec<bool> {
            self.children
                .lock()
                .unwrap()
                .iter()
                .map(|alive| *alive.lock().unwrap())
                .collect()
        }

        /// Every command spawned so far.
        pub fn spawned(&self) -> Vec<String> {
            self.spawned.lock().unwrap().clone()
        }

        fn key(command: &Command) -> String {
            let name = command
                .program
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            format!("{name} {}", command.args.join(" "))
                .trim()
                .to_owned()
        }
    }

    #[async_trait]
    impl ChildProcess for FakeChild {
        async fn kill(&self) -> Result<(), ProcessError> {
            *self.alive.lock().unwrap() = false;
            Ok(())
        }
        async fn is_alive(&self) -> bool {
            *self.alive.lock().unwrap()
        }
    }

    #[async_trait]
    impl CommandRunner for FakeRunner {
        async fn run(&self, command: &Command, timeout: Duration) -> Result<Output, ProcessError> {
            let key = Self::key(command);
            self.calls.lock().unwrap().push(key.clone());
            if self
                .timeouts
                .lock()
                .unwrap()
                .iter()
                .any(|prefix| key.starts_with(prefix.as_str()))
            {
                return Err(ProcessError::Timeout {
                    command: key,
                    timeout,
                });
            }
            let scripts = self.scripts.lock().unwrap();
            let hit = scripts
                .iter()
                .filter(|(prefix, _)| key.starts_with(prefix.as_str()))
                .max_by_key(|(prefix, _)| prefix.len())
                .map(|(_, out)| out.clone());
            // Unscripted commands succeed with empty output.
            Ok(hit.unwrap_or(Output {
                success: true,
                stdout: String::new(),
                stderr: String::new(),
            }))
        }

        async fn spawn(
            &self,
            command: &Command,
            _log_file: Option<&Path>,
        ) -> Result<Box<dyn ChildProcess>, ProcessError> {
            self.spawned.lock().unwrap().push(Self::key(command));
            let alive = Arc::new(Mutex::new(true));
            self.children.lock().unwrap().push(Arc::clone(&alive));
            Ok(Box::new(FakeChild { alive }))
        }
    }
}
