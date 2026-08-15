//! Host probing boundary: locating tools, reading environment variables and
//! running version commands. The real implementation touches the system;
//! tests inject a fake.

use std::path::{Path, PathBuf};
use std::time::Duration;

use async_trait::async_trait;

/// Captured output of a short-lived command.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CommandOutput {
    /// Whether the process exited successfully.
    pub success: bool,
    /// Standard output, UTF-8 lossy.
    pub stdout: String,
    /// Standard error, UTF-8 lossy.
    pub stderr: String,
}

impl CommandOutput {
    /// Both streams joined; handy for tools that print versions to stderr.
    pub fn combined(&self) -> String {
        format!("{}{}", self.stdout, self.stderr)
    }
}

/// Deadline for any diagnostic command.
pub const COMMAND_TIMEOUT: Duration = Duration::from_secs(30);

/// Read-only host access used by `mortsom doctor`.
#[async_trait]
pub trait HostProbe: Send + Sync {
    /// Environment variable lookup.
    fn env(&self, name: &str) -> Option<String>;
    /// Locates an executable on `PATH`.
    fn which(&self, program: &str) -> Option<PathBuf>;
    /// True when `path` exists (file or directory).
    fn exists(&self, path: &Path) -> bool;
    /// Runs a program with arguments and captures its output.
    async fn run(&self, program: &Path, args: &[&str]) -> Result<CommandOutput, String>;
}

/// The real host.
#[derive(Debug, Default, Clone, Copy)]
pub struct RealHost;

#[async_trait]
impl HostProbe for RealHost {
    fn env(&self, name: &str) -> Option<String> {
        std::env::var(name).ok().filter(|v| !v.is_empty())
    }

    fn which(&self, program: &str) -> Option<PathBuf> {
        let path = std::env::var_os("PATH")?;
        std::env::split_paths(&path)
            .map(|dir| dir.join(program))
            .find(|candidate| candidate.is_file())
    }

    fn exists(&self, path: &Path) -> bool {
        path.exists()
    }

    async fn run(&self, program: &Path, args: &[&str]) -> Result<CommandOutput, String> {
        let child = tokio::process::Command::new(program)
            .args(args)
            .kill_on_drop(true)
            .output();
        let output = tokio::time::timeout(COMMAND_TIMEOUT, child)
            .await
            .map_err(|_| format!("{} timed out after {COMMAND_TIMEOUT:?}", program.display()))?
            .map_err(|e| format!("cannot run {}: {e}", program.display()))?;
        Ok(CommandOutput {
            success: output.status.success(),
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        })
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn combined_joins_both_streams() {
        let output = CommandOutput {
            success: true,
            stdout: "a".into(),
            stderr: "b".into(),
        };
        assert_eq!(output.combined(), "ab");
    }

    #[tokio::test]
    async fn real_host_locates_sh_and_runs_it() {
        let host = RealHost;
        let sh = host.which("sh").expect("sh is on PATH");

        let output = host.run(&sh, &["-c", "echo hi"]).await.unwrap();

        assert!(output.success);
        assert_eq!(output.stdout.trim(), "hi");
        assert!(host.exists(&sh));
        assert!(host.which("definitely-not-a-program-xyz").is_none());
    }

    #[tokio::test]
    async fn real_host_reports_unrunnable_program() {
        let err = RealHost
            .run(Path::new("/nonexistent/bin"), &[])
            .await
            .unwrap_err();
        assert!(err.contains("cannot run"));
    }
}
