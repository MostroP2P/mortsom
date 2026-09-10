//! Run-owned `nostr-rs-relay` lifecycle (spec §16.3): unique name, port and
//! data directory per run; readiness probing; explicit teardown that only
//! ever touches this run's container.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use async_trait::async_trait;
use nostr_sdk::prelude::{Client, Filter, RelayUrl};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tracing::{info, warn};

use crate::config::RelayConfig;
use crate::docker::{ContainerRuntime, ContainerSpec, DockerError, RUN_LABEL};
use crate::ports::allocate_port;

/// Path inside the container where nostr-rs-relay keeps its database.
pub const RELAY_DATA_MOUNT: &str = "/usr/src/app/db";
/// Default readiness deadline.
pub const DEFAULT_READY_TIMEOUT: Duration = Duration::from_secs(60);
/// Interval between readiness probes.
pub const PROBE_INTERVAL: Duration = Duration::from_millis(500);
/// How many times a "port already allocated" race is retried with a fresh
/// port before giving up.
pub const PORT_RACE_RETRIES: u16 = 5;

/// A running relay owned by this run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RelayHandle {
    /// Container name (`<prefix>-<run_id>`).
    pub container_name: String,
    /// Host port the relay is published on.
    pub host_port: u16,
    /// Run-specific data directory bind-mounted into the container.
    pub data_dir: PathBuf,
    /// Docker image used.
    pub image: String,
}

impl RelayHandle {
    /// URL as seen from the host (runner, host-side daemon).
    pub fn host_url(&self) -> String {
        format!("ws://127.0.0.1:{}", self.host_port)
    }

    /// URL as seen from another Docker container (`host.docker.internal`).
    pub fn docker_url(&self) -> String {
        format!(
            "ws://{}:{}",
            crate::daemon::DOCKER_HOST_ALIAS,
            self.host_port
        )
    }
}

/// Relay lifecycle failures. Startup failures carry the container logs.
#[derive(Debug, Error)]
pub enum RelayError {
    /// No free host port in the search window.
    #[error("no free host port at or above {base}")]
    NoFreePort {
        /// Configured base port.
        base: u16,
    },
    /// The data directory could not be created.
    #[error("cannot create relay data directory {path}: {source}")]
    DataDir {
        /// Directory that failed.
        path: PathBuf,
        /// I/O error.
        #[source]
        source: std::io::Error,
    },
    /// The container runtime failed.
    #[error(transparent)]
    Docker(#[from] DockerError),
    /// The relay never answered a Nostr probe before the deadline.
    #[error(
        "relay {container_name} on port {host_port} not ready after {timeout:?}; logs:\n{logs}"
    )]
    NotReady {
        /// Container name.
        container_name: String,
        /// Host port.
        host_port: u16,
        /// Deadline used.
        timeout: Duration,
        /// Container logs captured before removal.
        logs: String,
    },
}

/// Answers whether a relay speaks Nostr at a URL. Injected so tests never
/// open sockets.
#[async_trait]
pub trait ReadinessProbe: Send + Sync {
    /// True when the relay accepted a connection and answered a REQ.
    async fn is_ready(&self, url: &str) -> bool;
}

/// Real probe: connect with `nostr-sdk` and issue a bounded fetch.
#[derive(Debug, Default, Clone, Copy)]
pub struct NostrProbe;

#[async_trait]
impl ReadinessProbe for NostrProbe {
    async fn is_ready(&self, url: &str) -> bool {
        let Ok(relay) = RelayUrl::parse(url) else {
            return false;
        };
        let client = Client::default();
        if client.add_relay(relay).await.is_err() {
            return false;
        }
        let output = client.try_connect().timeout(Duration::from_secs(3)).await;
        let connected = output.failed.is_empty() && !output.success.is_empty();
        let answered = connected
            && client
                .fetch_events(Filter::new().limit(1))
                .timeout(Duration::from_secs(3))
                .await
                .is_ok();
        client.disconnect().await;
        answered
    }
}

/// Starts the run's relay and waits until it answers Nostr requests. On any
/// failure after the container was created, its logs are captured and only
/// that container is removed. A lost race for the host port (another process
/// bound it between allocation and `docker run`) is retried with the next
/// free port up to [`PORT_RACE_RETRIES`] times.
pub async fn start_relay(
    runtime: &dyn ContainerRuntime,
    probe: &dyn ReadinessProbe,
    config: &RelayConfig,
    run_id: &str,
    run_dir: &Path,
    ready_timeout: Duration,
) -> Result<RelayHandle, RelayError> {
    let data_dir = run_dir.join("relay-data");
    std::fs::create_dir_all(&data_dir).map_err(|source| RelayError::DataDir {
        path: data_dir.clone(),
        source,
    })?;
    let container_name = format!("{}-{run_id}", config.container_name_prefix);
    let mut base = config.host_port;
    for _attempt in 0..=PORT_RACE_RETRIES {
        let host_port = allocate_port(base).ok_or(RelayError::NoFreePort {
            base: config.host_port,
        })?;
        if host_port != config.host_port {
            warn!(
                configured = config.host_port,
                allocated = host_port,
                "configured relay port busy; the daemon must be pointed at the allocated port"
            );
        }
        let spec = container_spec(config, &container_name, host_port, &data_dir, run_id);
        match runtime.run(&spec).await {
            Ok(_) => {}
            Err(DockerError::Command { stderr, .. }) if is_port_race(&stderr) => {
                warn!(
                    port = host_port,
                    "lost the host port race; retrying with the next free port"
                );
                // `docker run` can create the container and still fail to
                // publish the port, and the next attempt uses the same name.
                // Leaving it behind turns a retry into a name conflict.
                if let Err(e) = runtime.stop_and_remove(&container_name).await {
                    warn!(container = %container_name, error = %e, "cannot remove the container left by the lost port race");
                }
                base = host_port.saturating_add(1);
                continue;
            }
            Err(e) => return Err(e.into()),
        }
        let handle = RelayHandle {
            container_name: container_name.clone(),
            host_port,
            data_dir: data_dir.clone(),
            image: config.docker_image.clone(),
        };
        info!(container = %handle.container_name, port = host_port, "relay container started");
        return await_or_remove(runtime, probe, handle, ready_timeout).await;
    }
    Err(RelayError::NoFreePort {
        base: config.host_port,
    })
}

fn container_spec(
    config: &RelayConfig,
    name: &str,
    host_port: u16,
    data_dir: &Path,
    run_id: &str,
) -> ContainerSpec {
    ContainerSpec {
        name: name.to_owned(),
        image: config.docker_image.clone(),
        ports: vec![(host_port, config.container_port)],
        volumes: vec![(data_dir.to_path_buf(), RELAY_DATA_MOUNT.to_owned())],
        env: BTreeMap::new(),
        labels: [(RUN_LABEL.to_owned(), run_id.to_owned())]
            .into_iter()
            .collect(),
    }
}

fn is_port_race(stderr: &str) -> bool {
    stderr.contains("port is already allocated") || stderr.contains("address already in use")
}

async fn await_or_remove(
    runtime: &dyn ContainerRuntime,
    probe: &dyn ReadinessProbe,
    handle: RelayHandle,
    ready_timeout: Duration,
) -> Result<RelayHandle, RelayError> {
    if wait_ready(runtime, probe, &handle, ready_timeout).await {
        return Ok(handle);
    }
    let logs = runtime
        .logs(&handle.container_name)
        .await
        .unwrap_or_default();
    if let Err(e) = runtime.stop_and_remove(&handle.container_name).await {
        warn!(error = %e, "could not remove relay container after failed startup");
    }
    Err(RelayError::NotReady {
        container_name: handle.container_name,
        host_port: handle.host_port,
        timeout: ready_timeout,
        logs,
    })
}

async fn wait_ready(
    runtime: &dyn ContainerRuntime,
    probe: &dyn ReadinessProbe,
    handle: &RelayHandle,
    timeout: Duration,
) -> bool {
    let deadline = tokio::time::Instant::now() + timeout;
    let url = handle.host_url();
    loop {
        if probe.is_ready(&url).await {
            return true;
        }
        if !runtime
            .is_running(&handle.container_name)
            .await
            .unwrap_or(false)
        {
            return false;
        }
        if tokio::time::Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(PROBE_INTERVAL).await;
    }
}

/// Restarts the run relay in place and waits until it answers again. The
/// container keeps its name, port and data directory.
pub async fn restart_relay(
    runtime: &dyn ContainerRuntime,
    probe: &dyn ReadinessProbe,
    handle: &RelayHandle,
    ready_timeout: Duration,
) -> Result<(), RelayError> {
    runtime.restart(&handle.container_name).await?;
    if wait_ready(runtime, probe, handle, ready_timeout).await {
        return Ok(());
    }
    let logs = runtime
        .logs(&handle.container_name)
        .await
        .unwrap_or_default();
    Err(RelayError::NotReady {
        container_name: handle.container_name.clone(),
        host_port: handle.host_port,
        timeout: ready_timeout,
        logs,
    })
}

/// Result of tearing a relay down.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RelayTeardown {
    /// Container name.
    pub container_name: String,
    /// Whether the container was removed.
    pub removed: bool,
    /// Whether the data directory was deleted.
    pub data_removed: bool,
    /// Error text, if anything failed.
    pub error: Option<String>,
}

/// Captures logs, stops and removes exactly this run's container, and
/// deletes its data directory unless `keep_data` is set. Never touches
/// another container. The returned logs are raw; callers persist them only
/// through a redacting artifact writer.
pub async fn teardown_relay(
    runtime: &dyn ContainerRuntime,
    handle: &RelayHandle,
    keep_data: bool,
) -> (RelayTeardown, String) {
    let mut result = RelayTeardown {
        container_name: handle.container_name.clone(),
        removed: false,
        data_removed: false,
        error: None,
    };
    let logs = match runtime.logs(&handle.container_name).await {
        Ok(logs) => logs,
        Err(e) => {
            result.error = Some(format!("logs: {e}"));
            String::new()
        }
    };
    match runtime.stop_and_remove(&handle.container_name).await {
        Ok(()) => result.removed = true,
        Err(e) => result.error = Some(e.to_string()),
    }
    if !keep_data && result.removed {
        match std::fs::remove_dir_all(&handle.data_dir) {
            Ok(()) => result.data_removed = true,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => result.data_removed = true,
            Err(e) => result.error = Some(format!("data dir: {e}")),
        }
    }
    (result, logs)
}

pub mod replay;

#[cfg(test)]
mod tests;
