//! The public record `stack up` leaves behind: everything a run needs to
//! attach, nothing a run must not see.
use std::path::Path;

use serde::{Deserialize, Serialize};

use super::connections::Connections;
use super::layout::Layout;
use super::metadata::{DaemonImage, DeclaredLimits, StackSnapshot};
use super::{StackError, fingerprint::StackFingerprint};

/// Descriptor framing; bump when a field changes meaning.
pub const SCHEMA_VERSION: u32 = 1;

/// Written to `stack/local/public.json` at the end of `up`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Descriptor {
    /// Always [`SCHEMA_VERSION`] when written by this version.
    pub schema_version: u32,
    /// Hex public key minted for this stack.
    pub daemon_pubkey: String,
    /// Where the relay and the actor wallets answer.
    pub connections: Connections,
    /// The image the daemon was started from.
    pub daemon_image: DaemonImage,
    /// Limits declared in `stack/limits.toml` at `up` time.
    pub declared_limits: DeclaredLimits,
    /// Digest of the public snapshot, the same one runs record.
    pub fingerprint: StackFingerprint,
}

impl Descriptor {
    /// Assembles the descriptor and its fingerprint.
    pub fn new(
        daemon_pubkey: String,
        connections: Connections,
        daemon_image: DaemonImage,
        declared_limits: DeclaredLimits,
    ) -> Result<Self, StackError> {
        let snapshot = StackSnapshot {
            daemon_pubkey: daemon_pubkey.clone(),
            relay_url: connections.relay_url.clone(),
            wallet_endpoints: connections.wallet_endpoints(),
            daemon_image: daemon_image.clone(),
            declared_limits: declared_limits.clone(),
        };
        let fingerprint = snapshot
            .fingerprint()
            .map_err(|error| StackError::Configuration(error.to_owned()))?;
        Ok(Self {
            schema_version: SCHEMA_VERSION,
            daemon_pubkey,
            connections,
            daemon_image,
            declared_limits,
            fingerprint,
        })
    }

    /// Writes the descriptor to its place in `layout`.
    pub fn write(&self, layout: &Layout) -> Result<(), StackError> {
        let encoded = serde_json::to_string_pretty(self)
            .map_err(|error| StackError::Configuration(error.to_string()))?;
        std::fs::write(layout.descriptor_file(), encoded).map_err(|error| {
            StackError::Configuration(format!(
                "cannot write {}: {error}",
                layout.descriptor_file().display()
            ))
        })
    }

    /// `None` when `stack up` never completed here.
    pub fn read(layout: &Layout) -> Result<Option<Self>, StackError> {
        let path = layout.descriptor_file();
        let raw = match std::fs::read_to_string(&path) {
            Ok(raw) => raw,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => {
                return Err(StackError::Configuration(format!(
                    "cannot read {}: {error}",
                    path.display()
                )));
            }
        };
        let descriptor: Self = serde_json::from_str(&raw).map_err(|_| {
            StackError::Configuration(format!(
                "{} is not a descriptor this version understands; run `mortsom stack up` again",
                path.display()
            ))
        })?;
        if descriptor.schema_version != SCHEMA_VERSION {
            return Err(StackError::Configuration(format!(
                "{} was written by another version; run `mortsom stack up` again",
                path.display()
            )));
        }
        descriptor.daemon_image.validate()?;
        descriptor.declared_limits.validate()?;
        descriptor.connections.validate(layout.root())?;
        Ok(Some(descriptor))
    }
}

/// Reads the committed limits declaration.
pub fn load_limits(path: &Path) -> Result<DeclaredLimits, StackError> {
    let raw = std::fs::read_to_string(path).map_err(|error| {
        StackError::Configuration(format!("cannot read {}: {error}", path.display()))
    })?;
    let limits: DeclaredLimits = toml::from_str(&raw).map_err(|error| {
        StackError::Configuration(format!("{} is invalid: {error}", path.display()))
    })?;
    limits.validate()?;
    Ok(limits)
}
