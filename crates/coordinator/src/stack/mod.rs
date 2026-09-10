//! Mortsom's own regtest stack (spec section 5): a Compose project this
//! checkout creates, inspects and destroys, with nothing but Docker.

mod compose;
mod connections;
mod descriptor;
mod fingerprint;
mod image;
mod layout;
mod logs;
mod metadata;
mod nodes;
mod ownership;
mod status;
mod up;

pub use compose::Compose;
pub use connections::credential_reference;
pub use descriptor::Descriptor;
pub use fingerprint::StackFingerprint;
pub use layout::{Layout, PROJECT};
pub use metadata::{DaemonImage, DeclaredLimits, StackSnapshot};
pub use status::Status;
pub use up::{Timing, UpOptions, up};

use std::collections::BTreeMap;
use std::path::Path;
use std::time::Duration;

use async_trait::async_trait;
use mortsom_ui_driver::process::{CommandRunner, ProcessError};
use nostr_sdk::prelude::PublicKey;
use thiserror::Error;

/// Stack boundary failures. Child output is redacted before presentation.
#[derive(Debug, Error)]
pub enum StackError {
    /// Missing stack state, a refused mutation or an unusable service.
    #[error("stack is not configured: {0}; run `mortsom stack up` first")]
    Configuration(String),
    /// A request the stack cannot serve as asked; nothing is wrong with
    /// the stack itself.
    #[error("{0}")]
    Request(String),
    /// A Docker command failed.
    #[error(transparent)]
    Process(ProcessError),
}

/// The daemon release `stack up` installs unless the operator selects
/// another (spec section 5). A fixed tag keeps accepted runs reproducible:
/// a floating `latest` would swap the daemon under the harness whenever
/// upstream publishes, and with it the protocol behaviour being tested.
pub const DEFAULT_MOSTRO_IMAGE: &str = "mostrop2p/mostro:v0.18.7";

/// How `up` learns that the daemon is alive on the relay.
#[async_trait]
pub trait InfoProbe: Send + Sync {
    /// Whether `pubkey` has published its info event on `relay_url`.
    async fn daemon_published_info(&self, relay_url: &str, pubkey: PublicKey) -> bool;
}

/// Fetches the info event from the real relay.
pub struct LiveRelay;

const INFO_FETCH_TIMEOUT: Duration = Duration::from_secs(10);

#[async_trait]
impl InfoProbe for LiveRelay {
    async fn daemon_published_info(&self, relay_url: &str, pubkey: PublicKey) -> bool {
        mortsom_environment::daemon::probe_daemon(
            &[relay_url.to_owned()],
            pubkey,
            INFO_FETCH_TIMEOUT,
        )
        .await
        .info
        .is_some()
    }
}

/// Non-secret connection references of a usable stack.
#[derive(Debug)]
pub struct StackContext {
    values: BTreeMap<String, String>,
    relay_url: String,
    relay_port: u16,
    /// Image the daemon was started from and verified against.
    pub daemon_image: DaemonImage,
    /// Operator expectations; scenario observations must verify them.
    pub declared_limits: DeclaredLimits,
}

impl StackContext {
    /// Attaches to the stack in `dir`; refuses one that is not ready.
    pub async fn resolve(dir: &Path, runner: &dyn CommandRunner) -> Result<Self, StackError> {
        let layout = Layout::new(dir)?;
        ownership::verify(runner, &layout).await?;
        let status = status::inspect(runner, &layout).await?;
        let Some(descriptor) = status.descriptor.clone() else {
            return Err(StackError::Configuration(
                "no daemon identity is recorded here".into(),
            ));
        };
        if !status.is_ready() {
            return Err(StackError::Configuration(format!(
                "the stack is not ready:\n{}",
                status.render()
            )));
        }
        let key = PublicKey::parse(&descriptor.daemon_pubkey)
            .map_err(|_| StackError::Configuration("invalid daemon public key".into()))?;
        let relay_url = descriptor.connections.relay_url.clone();
        let mut values = descriptor.connections.values();
        values.insert("MORTSOM_MOSTRO_PUBKEY".into(), key.to_hex());
        values.insert(
            "MORTSOM_MOSTRO_SETTINGS_FILE".into(),
            layout.settings_file().display().to_string(),
        );
        Ok(Self {
            values,
            relay_url,
            relay_port: connections::RELAY_PORT,
            daemon_image: descriptor.daemon_image,
            declared_limits: descriptor.declared_limits,
        })
    }

    /// Captures only public facts; credential paths stay in memory.
    pub fn snapshot(&self) -> StackSnapshot {
        StackSnapshot {
            daemon_pubkey: self.values["MORTSOM_MOSTRO_PUBKEY"].clone(),
            relay_url: self.relay_url.clone(),
            wallet_endpoints: connections::ACTOR_PORTS
                .into_iter()
                .map(|(actor, _)| {
                    let key = format!("MORTSOM_{}_LND_HOST", actor.to_uppercase());
                    (actor.into(), self.values[&key].clone())
                })
                .collect(),
            daemon_image: self.daemon_image.clone(),
            declared_limits: self.declared_limits.clone(),
        }
    }

    /// Resolves one documented connection reference.
    pub fn lookup(&self, name: &str) -> Option<String> {
        self.values.get(name).cloned()
    }

    /// Applies the actual relay binding to shared configuration, the
    /// untaken-order lifetime the stack declares, which the client build
    /// asks the daemon for on every order it creates, and the daemon's
    /// payout limits, which the payment-failure family drives.
    pub fn configure_relay(&self, config: &mut crate::Config) {
        config.relay.host_port = self.relay_port;
        config.mostro.relays = vec![self.relay_url.clone()];
        config.mobile.order_expiry_secs = Some(self.declared_limits.order_expiration_secs);
        config.run.payout_limits = self.declared_limits.payout_limits();
    }
}

/// Inspects the stack without changing it.
pub async fn status(runner: &dyn CommandRunner, layout: &Layout) -> Result<Status, StackError> {
    ownership::verify(runner, layout).await?;
    status::inspect(runner, layout).await
}

pub use logs::{LogsOptions, logs, services as log_services};

/// Stops the services, removes their volumes and deletes every generated
/// credential and identity. Refuses containers another directory created.
pub async fn down(runner: &dyn CommandRunner, layout: &Layout) -> Result<(), StackError> {
    ownership::verify(runner, layout).await?;
    tracing::info!("tearing the stack down");
    let compose = Compose::new(layout, None);
    compose::require(
        runner,
        &compose.command(&["--profile", "daemon", "down", "-v", "--remove-orphans"]),
        Duration::from_secs(600),
    )
    .await?;
    up::remove_dir(&layout.local_dir())
}

#[cfg(test)]
mod tests;

impl From<ProcessError> for StackError {
    fn from(error: ProcessError) -> Self {
        let redact = mortsom_common::Redactor::default();
        let error = match error {
            ProcessError::Spawn { command, message } => ProcessError::Spawn {
                command: redact.redact(&command),
                message: redact.redact(&message),
            },
            ProcessError::Timeout { command, timeout } => ProcessError::Timeout {
                command: redact.redact(&command),
                timeout,
            },
            ProcessError::Failed { command, stderr } => ProcessError::Failed {
                command: redact.redact(&command),
                stderr: redact.redact(&stderr),
            },
        };
        Self::Process(error)
    }
}
