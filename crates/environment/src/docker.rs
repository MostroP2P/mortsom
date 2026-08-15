//! Container runtime boundary. The real implementation shells out to the
//! `docker` CLI through `tokio::process`; tests inject a fake.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::Duration;

use async_trait::async_trait;
use thiserror::Error;

/// Deadline for any single docker CLI invocation.
pub const DOCKER_TIMEOUT: Duration = Duration::from_secs(120);

/// Label attached to every Mortsom container, holding the run id.
pub const RUN_LABEL: &str = "mortsom.run";

/// What to run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContainerSpec {
    /// Unique container name.
    pub name: String,
    /// Image reference.
    pub image: String,
    /// `(host_port, container_port)` publications on all host interfaces.
    pub ports: Vec<(u16, u16)>,
    /// `(host_path, container_path)` bind mounts.
    pub volumes: Vec<(PathBuf, String)>,
    /// Environment variables inside the container.
    pub env: BTreeMap<String, String>,
    /// Labels; [`RUN_LABEL`] is always set by callers.
    pub labels: BTreeMap<String, String>,
}

impl ContainerSpec {
    /// The `docker run` argument list for this spec.
    pub fn run_args(&self) -> Vec<String> {
        let mut args = vec![
            "run".to_owned(),
            "-d".to_owned(),
            "--name".to_owned(),
            self.name.clone(),
        ];
        for (host, container) in &self.ports {
            args.push("-p".to_owned());
            args.push(format!("{host}:{container}"));
        }
        for (host, container) in &self.volumes {
            args.push("-v".to_owned());
            args.push(format!("{}:{container}", host.display()));
        }
        for (key, value) in &self.env {
            args.push("-e".to_owned());
            args.push(format!("{key}={value}"));
        }
        for (key, value) in &self.labels {
            args.push("--label".to_owned());
            args.push(format!("{key}={value}"));
        }
        args.push(self.image.clone());
        args
    }
}

/// Container runtime failures.
#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum DockerError {
    /// The docker CLI could not be executed.
    #[error("cannot run docker: {0}")]
    Unavailable(String),
    /// A docker command exited unsuccessfully.
    #[error("docker {command} failed: {stderr}")]
    Command {
        /// Sub-command that failed.
        command: String,
        /// Trimmed stderr.
        stderr: String,
    },
    /// A docker command exceeded [`DOCKER_TIMEOUT`].
    #[error("docker {0} timed out")]
    Timeout(String),
}

/// Minimal container operations Mortsom needs.
#[async_trait]
pub trait ContainerRuntime: Send + Sync {
    /// Starts a detached container and returns its id.
    async fn run(&self, spec: &ContainerSpec) -> Result<String, DockerError>;
    /// Whether a container with this exact name is running.
    async fn is_running(&self, name: &str) -> Result<bool, DockerError>;
    /// Stops and removes the container with this exact name. Removing a
    /// container that does not exist is not an error.
    async fn stop_and_remove(&self, name: &str) -> Result<(), DockerError>;
    /// Restarts a running container in place (same name, ports and data).
    async fn restart(&self, name: &str) -> Result<(), DockerError>;
    /// Container logs (stdout and stderr).
    async fn logs(&self, name: &str) -> Result<String, DockerError>;
    /// Names of containers (running or not) whose name starts with `prefix`.
    async fn list_names(&self, prefix: &str) -> Result<Vec<String>, DockerError>;
}

/// The `docker` CLI.
#[derive(Debug, Clone)]
pub struct DockerCli {
    program: PathBuf,
}

impl Default for DockerCli {
    fn default() -> Self {
        Self {
            program: PathBuf::from("docker"),
        }
    }
}

impl DockerCli {
    /// Uses a specific docker executable.
    pub fn with_program(program: PathBuf) -> Self {
        Self { program }
    }

    async fn exec(&self, args: &[String]) -> Result<String, DockerError> {
        let command = args.first().cloned().unwrap_or_default();
        let output = tokio::time::timeout(
            DOCKER_TIMEOUT,
            tokio::process::Command::new(&self.program)
                .args(args)
                .kill_on_drop(true)
                .output(),
        )
        .await
        .map_err(|_| DockerError::Timeout(command.clone()))?
        .map_err(|e| DockerError::Unavailable(e.to_string()))?;
        if output.status.success() {
            Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
        } else {
            Err(DockerError::Command {
                command,
                stderr: String::from_utf8_lossy(&output.stderr).trim().to_owned(),
            })
        }
    }
}

#[async_trait]
impl ContainerRuntime for DockerCli {
    async fn run(&self, spec: &ContainerSpec) -> Result<String, DockerError> {
        self.exec(&spec.run_args()).await
    }

    async fn is_running(&self, name: &str) -> Result<bool, DockerError> {
        let out = self
            .exec(&[
                "ps".into(),
                "--filter".into(),
                format!("name=^/{name}$"),
                "--filter".into(),
                "status=running".into(),
                "--format".into(),
                "{{.Names}}".into(),
            ])
            .await?;
        Ok(out.lines().any(|line| line.trim() == name))
    }

    async fn stop_and_remove(&self, name: &str) -> Result<(), DockerError> {
        match self.exec(&["rm".into(), "-f".into(), name.into()]).await {
            Ok(_) => Ok(()),
            Err(DockerError::Command { stderr, .. }) if stderr.contains("No such container") => {
                Ok(())
            }
            Err(e) => Err(e),
        }
    }

    async fn restart(&self, name: &str) -> Result<(), DockerError> {
        self.exec(&["restart".into(), name.into()])
            .await
            .map(|_| ())
    }

    async fn logs(&self, name: &str) -> Result<String, DockerError> {
        let output = tokio::time::timeout(
            DOCKER_TIMEOUT,
            tokio::process::Command::new(&self.program)
                .args(["logs", name])
                .kill_on_drop(true)
                .output(),
        )
        .await
        .map_err(|_| DockerError::Timeout("logs".into()))?
        .map_err(|e| DockerError::Unavailable(e.to_string()))?;
        Ok(format!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        ))
    }

    async fn list_names(&self, prefix: &str) -> Result<Vec<String>, DockerError> {
        // The name filter is a regex. `prefix` is regex-safe only because
        // `RelayConfig::validate` restricts it to `[A-Za-z0-9._-]`; the
        // post-filter below re-checks the prefix literally regardless.
        let out = self
            .exec(&[
                "ps".into(),
                "-a".into(),
                "--filter".into(),
                format!("name=^/{prefix}"),
                "--format".into(),
                "{{.Names}}".into(),
            ])
            .await?;
        Ok(out
            .lines()
            .map(str::trim)
            .filter(|line| line.starts_with(prefix))
            .map(str::to_owned)
            .collect())
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn run_args_follow_docker_cli_order() {
        let spec = ContainerSpec {
            name: "mortsom-relay-r1".into(),
            image: "scsibug/nostr-rs-relay:latest".into(),
            ports: vec![(7001, 8080)],
            volumes: vec![(PathBuf::from("/tmp/data"), "/usr/src/app/db".into())],
            env: [("RUST_LOG".to_owned(), "info".to_owned())]
                .into_iter()
                .collect(),
            labels: [(RUN_LABEL.to_owned(), "r1".to_owned())]
                .into_iter()
                .collect(),
        };

        let args = spec.run_args();

        assert_eq!(
            args,
            vec![
                "run",
                "-d",
                "--name",
                "mortsom-relay-r1",
                "-p",
                "7001:8080",
                "-v",
                "/tmp/data:/usr/src/app/db",
                "-e",
                "RUST_LOG=info",
                "--label",
                "mortsom.run=r1",
                "scsibug/nostr-rs-relay:latest"
            ]
        );
    }

    #[tokio::test]
    async fn missing_docker_binary_is_unavailable() {
        let cli = DockerCli::with_program(PathBuf::from("/nonexistent/docker"));

        let err = cli.is_running("x").await.unwrap_err();

        assert!(matches!(err, DockerError::Unavailable(_)));
    }

    #[tokio::test]
    async fn stop_and_remove_tolerates_missing_container() {
        // `false` exits 1 with empty stderr; a "No such container" stderr is
        // treated as success, anything else is an error. Use a shell shim.
        let dir = tempfile::tempdir().unwrap();
        let shim = dir.path().join("docker");
        std::fs::write(
            &shim,
            "#!/bin/sh\necho 'Error: No such container: x' >&2\nexit 1\n",
        )
        .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&shim, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let cli = DockerCli::with_program(shim);

        assert!(cli.stop_and_remove("x").await.is_ok());
        let err = cli
            .run(&ContainerSpec {
                name: "n".into(),
                image: "i".into(),
                ports: vec![],
                volumes: vec![],
                env: BTreeMap::new(),
                labels: BTreeMap::new(),
            })
            .await
            .unwrap_err();
        assert!(matches!(err, DockerError::Command { .. }));
    }
}
