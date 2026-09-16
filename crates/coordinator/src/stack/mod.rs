//! Mortsom's own regtest stack (spec section 5): a Compose project this
//! checkout creates, inspects and destroys, with nothing but Docker.

mod compose;
mod connections;
mod daemon;
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
pub use daemon::DaemonSource;
pub use descriptor::Descriptor;
pub use fingerprint::StackFingerprint;
pub use image::{LOCAL_REPOSITORY, commit_tag};
pub use layout::{Layout, PROJECT};
pub use metadata::{DaemonImage, DaemonProvenance, DeclaredLimits, StackSnapshot};
pub use ownership::ProjectOwner;
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
    #[error("stack is not configured: {0}")]
    Configuration(String),
    /// The Compose project exists, but another stack directory created it;
    /// holds that directory. Neither `up` nor `down` here may touch it.
    #[error(
        "the `{project}` Compose project belongs to {0}, not this stack directory; \
         run `mortsom stack down --stack-dir {0}` to release it, or \
         `mortsom stack up --replace` to remove it and build here",
        project = PROJECT
    )]
    ForeignOwner(String),
    /// Project resources exist that cannot be shown to belong to this stack
    /// directory; holds why.
    #[error(
        "the `{project}` Compose project cannot be shown to belong to this stack directory: \
         {0}; `mortsom stack up --replace` removes it and builds here",
        project = PROJECT
    )]
    Unowned(String),
    /// A request the stack cannot serve as asked; nothing is wrong with
    /// the stack itself.
    #[error("{0}")]
    Request(String),
    /// A Docker command failed.
    #[error(transparent)]
    Process(ProcessError),
    /// The selected daemon source could not be fetched or verified.
    #[error(transparent)]
    Source(#[from] crate::git_source::SourceError),
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
    /// How that image was selected, and the commit when it was built here.
    pub daemon_source: DaemonProvenance,
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
                "no daemon identity is recorded here; run `mortsom stack up` first".into(),
            ));
        };
        if !status.is_ready() {
            return Err(StackError::Configuration(format!(
                "the stack is not ready; run `mortsom stack up` to finish it:\n{}",
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
            daemon_source: descriptor.daemon_source,
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
            daemon_source: self.daemon_source.clone(),
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

/// Which directory holds the Compose project, read without changing it.
pub async fn project_owner(runner: &dyn CommandRunner) -> Result<ProjectOwner, StackError> {
    ownership::owner(runner).await
}

/// The stack identity whose generated state lives in a stack directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RecordedIdentity {
    /// No generated state exists there: never built, or torn down.
    Absent,
    /// A completed `stack up` left this identity.
    Present(StackFingerprint),
    /// Generated state exists but its identity cannot be read; holds why.
    Unreadable(String),
}

/// Reads the identity `stack up` recorded in `dir` without touching Docker.
pub fn recorded_identity(dir: &Path) -> RecordedIdentity {
    identity_using(dir, |descriptor| Ok(descriptor.fingerprint))
}

fn identity_using(
    dir: &Path,
    fingerprint: impl FnOnce(Descriptor) -> Result<StackFingerprint, String>,
) -> RecordedIdentity {
    if !dir.join("local").exists() {
        return RecordedIdentity::Absent;
    }
    let layout = match Layout::new(dir) {
        Ok(layout) => layout,
        Err(error) => return RecordedIdentity::Unreadable(error.to_string()),
    };
    match Descriptor::read(&layout) {
        Ok(Some(descriptor)) => match fingerprint(descriptor) {
            Ok(value) => RecordedIdentity::Present(value),
            Err(error) => RecordedIdentity::Unreadable(error),
        },
        Ok(None) => RecordedIdentity::Unreadable(format!(
            "{} holds generated state without a completed descriptor",
            layout.root().display()
        )),
        Err(error) => RecordedIdentity::Unreadable(error.to_string()),
    }
}

/// Read-only schema-compatible comparison for legacy recovery guards.
pub(crate) fn recorded_identity_for(dir: &Path, original: &StackFingerprint) -> RecordedIdentity {
    identity_using(dir, |descriptor| {
        let snapshot = descriptor.snapshot();
        match original.algorithm.as_str() {
            "sha256-mortsom-stack-v1" => snapshot.legacy_fingerprint(),
            "sha256-mortsom-stack-v2" => snapshot.fingerprint(),
            _ => Err("unsupported recovery stack fingerprint algorithm".into()),
        }
    })
}

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
