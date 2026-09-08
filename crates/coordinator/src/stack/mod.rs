//! Attachment to the persistent regtest stack owned by the Ortsom CLI.

mod connections;
pub use connections::credential_reference;
mod fingerprint;
mod image;
pub use fingerprint::StackFingerprint;
mod metadata;
pub use metadata::{DaemonImage, DeclaredLimits, StackSnapshot};
mod ownership;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use mortsom_android_driver::process::{Command, CommandRunner, ProcessError, run_ok};
use nostr_sdk::prelude::PublicKey;
use thiserror::Error;

/// Stack boundary failures. Child output is redacted before presentation.
#[derive(Debug, Error)]
pub enum StackError {
    /// Missing stack metadata or invalid public identity.
    #[error("stack is not configured: {0}; run `mortsom stack up` first")]
    Configuration(String),
    /// External lifecycle command failed.
    #[error(transparent)]
    Process(ProcessError),
}

/// Non-secret connection references exported by an existing Ortsom stack.
#[derive(Debug)]
pub struct StackContext {
    values: BTreeMap<String, String>,
    relay_url: String,
    relay_port: u16,
    /// Actual running container image identity.
    pub daemon_image: DaemonImage,
    /// Operator expectations; scenario observations must verify them.
    pub declared_limits: DeclaredLimits,
}

#[derive(serde::Deserialize)]
struct PublicStatus {
    schema_version: u32,
    ready: bool,
    daemon_pubkey: Option<String>,
    connections: connections::Connections,
    daemon_image: DaemonImage,
    declared_limits: DeclaredLimits,
}

impl StackContext {
    /// Queries the versioned public CLI status, never a private settings file.
    pub async fn resolve(checkout: &Path, runner: &dyn CommandRunner) -> Result<Self, StackError> {
        let root = checkout
            .canonicalize()
            .map_err(|e| StackError::Configuration(e.to_string()))?;
        ownership::verify(runner, &root).await?;
        let mut command = stack_command(&root, &binary(&root), "status");
        command.args.push("--json".into());
        let output = run_ok(runner, &command, Duration::from_secs(60)).await?;
        let descriptor: PublicStatus = serde_json::from_str(&output.stdout).map_err(|_| {
            StackError::Configuration(
                "invalid or incomplete public status; rebuild Ortsom with public endpoint, image and limit metadata support".into(),
            )
        })?;
        if descriptor.schema_version != 1 || !descriptor.ready {
            return Err(StackError::Configuration(
                "unsupported or unready public stack status".into(),
            ));
        }
        descriptor.daemon_image.validate()?;
        descriptor.declared_limits.validate()?;
        let key = descriptor
            .daemon_pubkey
            .as_deref()
            .and_then(|s| PublicKey::parse(s).ok())
            .ok_or_else(|| StackError::Configuration("missing daemon public key".into()))?;
        let relay_url = descriptor.connections.relay_url.clone();
        let (relay_port, mut values) = descriptor.connections.resolve(&root)?;
        values.extend([
            ("MORTSOM_MOSTRO_PUBKEY".into(), key.to_hex()),
            (
                "MORTSOM_MOSTRO_SETTINGS_FILE".into(),
                root.join("ci/regtest/config/settings.toml")
                    .display()
                    .to_string(),
            ),
        ]);
        Ok(Self {
            values,
            relay_url,
            relay_port,
            daemon_image: descriptor.daemon_image,
            declared_limits: descriptor.declared_limits,
        })
    }

    /// Captures only public facts; private credential references stay in memory.
    pub fn snapshot(&self) -> StackSnapshot {
        StackSnapshot {
            daemon_pubkey: self.values["MORTSOM_MOSTRO_PUBKEY"].clone(),
            relay_url: self.relay_url.clone(),
            wallet_endpoints: ["alice", "bob"]
                .into_iter()
                .map(|actor| {
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

    /// Applies the actual persistent relay binding to shared configuration.
    pub fn configure_relay(&self, config: &mut crate::Config) {
        config.relay.host_port = self.relay_port;
        config.mostro.relays = vec![self.relay_url.clone()];
    }
}

/// Explicit command plan; the external CLI owns all stack mutations.
pub fn stack_command(checkout: &Path, binary: &Path, action: &str) -> Command {
    Command::new(
        "env",
        &[
            &format!("--chdir={}", checkout.display()),
            "COMPOSE_PROJECT_NAME=ortsom-regtest",
            &binary.display().to_string(),
            "stack",
            action,
        ],
    )
}

/// Invokes one Ortsom stack action, without automatic cleanup on failure.
pub async fn invoke(
    runner: &dyn CommandRunner,
    checkout: &Path,
    binary: &Path,
    action: &str,
) -> Result<String, StackError> {
    let root = checkout
        .canonicalize()
        .map_err(|e| StackError::Configuration(e.to_string()))?;
    let checkout = root.as_path();
    if matches!(action, "up" | "down") {
        ownership::verify(runner, checkout).await?;
    }
    let mut cmd = stack_command(checkout, binary, action);
    let mut expected = None;
    if action == "up" {
        // Always resolve the published latest image before creating services.
        run_ok(
            runner,
            &Command::new("docker", &["pull", "mostrop2p/mostro:latest"]),
            Duration::from_secs(600),
        )
        .await?;
        let image = image::resolve_latest(runner).await?;
        cmd.args
            .extend(["--mostro-image".into(), image.reference.clone()]);
        expected = Some(image);
    }
    let out = run_ok(runner, &cmd, Duration::from_secs(1800)).await?;
    if let Some(expected) = expected {
        image::verify_running(runner, checkout, &expected).await?;
        StackContext::resolve(checkout, runner).await?;
    }
    Ok(mortsom_common::Redactor::default().redact(&out.stdout))
}

/// Finds a built sibling CLI without building or importing its source.
pub fn binary(checkout: &Path) -> PathBuf {
    for profile in ["release", "debug"] {
        let path = checkout.join("target").join(profile).join("ortsom");
        if let Ok(path) = path.canonicalize() {
            return path;
        }
    }
    PathBuf::from("ortsom")
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
