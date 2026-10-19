//! The public record `stack up` leaves behind: everything a run needs to
//! attach, nothing a run must not see.
use std::path::Path;

use serde::{Deserialize, Serialize};

use super::connections::Connections;
use super::layout::Layout;
use super::metadata::{DaemonImage, DaemonProvenance, DeclaredLimits, StackSnapshot};
use super::{StackError, fingerprint::StackFingerprint};

/// Descriptor framing; bump when a field changes meaning. Version 2 added
/// the daemon's source provenance.
pub const SCHEMA_VERSION: u32 = 2;

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
    /// How the image was selected, and the commit when it was built here.
    #[serde(default)]
    pub daemon_source: DaemonProvenance,
    /// Limits declared in `stack/limits.toml` at `up` time.
    pub declared_limits: DeclaredLimits,
    /// Digest of the public snapshot, the same one runs record.
    pub fingerprint: StackFingerprint,
}

impl Descriptor {
    /// Public facts, retaining explicitly unrecorded source provenance in v1.
    pub fn snapshot(&self) -> StackSnapshot {
        StackSnapshot {
            daemon_pubkey: self.daemon_pubkey.clone(),
            relay_url: self.connections.relay_url.clone(),
            wallet_endpoints: self.connections.wallet_endpoints(),
            daemon_image: self.daemon_image.clone(),
            daemon_source: self.daemon_source.clone(),
            declared_limits: self.declared_limits.clone(),
        }
    }
    /// Assembles the descriptor and its fingerprint.
    pub fn new(
        daemon_pubkey: String,
        connections: Connections,
        daemon_image: DaemonImage,
        daemon_source: DaemonProvenance,
        declared_limits: DeclaredLimits,
    ) -> Result<Self, StackError> {
        let snapshot = StackSnapshot {
            daemon_pubkey: daemon_pubkey.clone(),
            relay_url: connections.relay_url.clone(),
            wallet_endpoints: connections.wallet_endpoints(),
            daemon_image: daemon_image.clone(),
            daemon_source: daemon_source.clone(),
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
            daemon_source,
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
        if ![1, SCHEMA_VERSION].contains(&descriptor.schema_version) {
            return Err(StackError::Configuration(format!(
                "{} was written by another version; run `mortsom stack up` again",
                path.display()
            )));
        }
        descriptor.daemon_image.validate()?;
        if descriptor.schema_version == SCHEMA_VERSION {
            descriptor
                .daemon_source
                .validate(&descriptor.daemon_image)?;
        } else if descriptor.daemon_source != DaemonProvenance::Unrecorded {
            return Err(StackError::Configuration(
                "legacy descriptor must not invent daemon provenance".into(),
            ));
        }
        descriptor.declared_limits.validate()?;
        descriptor.connections.validate(layout.root())?;
        let expected_algorithm = format!("sha256-mortsom-stack-v{}", descriptor.schema_version);
        if descriptor.fingerprint.algorithm != expected_algorithm
            || !descriptor
                .snapshot()
                .matches_recorded(&descriptor.fingerprint)
                .map_err(StackError::Configuration)?
        {
            return Err(StackError::Configuration(
                "stack descriptor fingerprint does not match its recorded facts".into(),
            ));
        }
        Ok(Some(descriptor))
    }
}

/// `stack/limits.toml`: the limits every stack declares, and the
/// untaken-order lifetime a bond stack declares in place of the plain one.
#[derive(Deserialize)]
struct LimitsFile {
    #[serde(flatten)]
    limits: DeclaredLimits,
    /// Absent from a shared stack directory (`--stack-dir`) of a checkout
    /// older than it; only a bond stack needs it.
    bond_order_expiration_secs: Option<u64>,
}

/// Reads the committed limits declaration for a stack with or without the
/// bond (`stack up --bond`).
pub fn load_limits(path: &Path, bond: bool) -> Result<DeclaredLimits, StackError> {
    let raw = std::fs::read_to_string(path).map_err(|error| {
        StackError::Configuration(format!("cannot read {}: {error}", path.display()))
    })?;
    let file: LimitsFile = toml::from_str(&raw).map_err(|error| {
        StackError::Configuration(format!("{} is invalid: {error}", path.display()))
    })?;
    let limits = if bond {
        // Never the plain lifetime instead: a slashed order outlives it.
        let order_expiration_secs = file.bond_order_expiration_secs.ok_or_else(|| {
            StackError::Configuration(format!(
                "{} declares no bond_order_expiration_secs, the untaken-order lifetime of a bond stack",
                path.display()
            ))
        })?;
        DeclaredLimits {
            order_expiration_secs,
            ..file.limits
        }
    } else {
        file.limits
    };
    limits.validate()?;
    Ok(limits)
}
