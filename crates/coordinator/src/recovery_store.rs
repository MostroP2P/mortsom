//! Operator-private recovery metadata; actor credentials never enter artifacts.
pub use crate::recovery_outcome::{
    AbandonReason, RecoveryEnvironment, RecoveryOutcome, WalletBackend,
};
use crate::{fingerprint::BundleFingerprint, provenance::BuildProvenance, stack::StackFingerprint};
use mortsom_common::RunId;
use mortsom_desktop_driver::Platform;
use mortsom_scenario_kit::journal::{RecoveryEvent, RecoveryJournal};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::Mutex,
};

/// Public facts necessary to resume the original UI identities.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecoveryManifest {
    /// Original invocation.
    pub run_id: RunId,
    /// Immutable stack identity.
    pub stack: StackFingerprint,
    /// Client runtime.
    pub platform: Platform,
    /// Captured bytes.
    pub bundle: BundleFingerprint,
    /// Build provenance.
    pub build: BuildProvenance,
    /// Native executable filename, absent for Web.
    pub executable: Option<String>,
    /// Observed original public actor identities.
    pub identities: BTreeMap<String, String>,
    /// Bound Web origin; must be reused after a restart.
    pub origin: Option<String>,
    /// Durable pre-action history, containing hashes only.
    pub events: Vec<RecoveryEvent>,
    /// True only after verified order/payment and process cleanup.
    pub closed: bool,
    /// Orders, wallets and process shutdown were verified before profile deletion.
    #[serde(default)]
    pub resolution_verified: bool,
    /// Terminal state; `None` only in manifests written before outcomes
    /// existed, where `closed` decides. Changed only by journal transitions.
    #[serde(default)]
    pub outcome: Option<RecoveryOutcome>,
    /// Recorded ownership, lane and wallet facts; `None` in older manifests,
    /// which therefore can never be abandoned.
    #[serde(default)]
    pub environment: Option<RecoveryEnvironment>,
}

impl RecoveryManifest {
    /// The journal's state, deriving it from `closed` for older manifests.
    pub fn state(&self) -> RecoveryOutcome {
        self.outcome.clone().unwrap_or(if self.closed {
            RecoveryOutcome::Resolved
        } else {
            RecoveryOutcome::Pending
        })
    }

    fn validate(&self) -> Result<(), String> {
        if self.state().is_pending() == self.closed {
            return Err("recovery manifest outcome contradicts its closed flag".into());
        }
        if matches!(self.outcome, Some(RecoveryOutcome::AbandonedRegtest { .. }))
            && self.resolution_verified
        {
            return Err("an abandoned recovery cannot claim verified resolution".into());
        }
        Ok(())
    }
}

/// Why a new desktop scenario cannot start yet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PendingRecovery {
    /// A previous run still holds identities that `mortsom cleanup` must resolve.
    Unresolved(RunId),
    /// Missing historical environment facts; only explicit regtest isolation is possible.
    Legacy(RunId),
    /// The recovery directory could not be inspected.
    Io(String),
}
impl PendingRecovery {
    /// Operator-facing text; `color` wraps the command to run in bold so it
    /// stands out on a terminal, and stays plain for logs and pipes.
    pub fn message(&self, color: bool) -> String {
        const BOLD: &str = "\u{1b}[1m";
        const RESET: &str = "\u{1b}[0m";
        match self {
            Self::Legacy(run_id) => format!(
                "{}\n{}",
                Self::Unresolved(run_id.clone()).message(color),
                crate::recovery_isolation::legacy_hint(run_id)
            ),
            Self::Unresolved(run_id) => {
                let command = format!("mortsom cleanup {run_id}");
                let highlighted = if color {
                    format!("{BOLD}{command}{RESET}")
                } else {
                    format!("`{command}`")
                };
                format!(
                    "run {run_id} retains unresolved recovery state; run {highlighted} before starting another desktop scenario"
                )
            }
            Self::Io(cause) => cause.clone(),
        }
    }
}
impl From<PendingRecovery> for String {
    fn from(pending: PendingRecovery) -> Self {
        pending.message(false)
    }
}

/// Durable journal with a caller-owned directory, never automatically removed.
#[derive(Debug)]
pub struct FileJournal {
    root: PathBuf,
    manifest: Mutex<RecoveryManifest>,
}
impl FileJournal {
    /// Resolve independent of artifact directory and checkout location.
    pub fn directory(home: &Path) -> Result<PathBuf, String> {
        home.canonicalize()
            .map(|home| home.join(".local/state/mortsom/recovery"))
            .map_err(|e| e.to_string())
    }
    /// Refuse new work while any previous protected identity needs recovery.
    pub fn refuse_pending(base: &Path) -> Result<(), PendingRecovery> {
        match Self::pending(base).map_err(PendingRecovery::Io)?.first() {
            Some(manifest) if manifest.environment.is_none() => {
                Err(PendingRecovery::Legacy(manifest.run_id.clone()))
            }
            Some(manifest) => Err(PendingRecovery::Unresolved(manifest.run_id.clone())),
            None => Ok(()),
        }
    }
    /// Read-only inventory of every journal that still blocks new work,
    /// oldest run first. Any unreadable journal fails the whole inventory.
    pub fn pending(base: &Path) -> Result<Vec<RecoveryManifest>, String> {
        if !base.exists() {
            return Ok(Vec::new());
        }
        let mut pending = Vec::new();
        for entry in std::fs::read_dir(base).map_err(|e| e.to_string())? {
            let entry = entry.map_err(|e| e.to_string())?;
            let manifest = Self::open(&entry.path())?.snapshot();
            if manifest.state().is_pending() {
                pending.push(manifest);
            }
        }
        pending.sort_by(|a, b| a.run_id.as_str().cmp(b.run_id.as_str()));
        Ok(pending)
    }
    /// Create metadata before actor startup; no overwrite of an existing run.
    pub fn create(base: &Path, manifest: RecoveryManifest) -> Result<Self, String> {
        crate::artifacts::persist::create_directory(base).map_err(|e| e.to_string())?;
        let root = base.join(manifest.run_id.as_str());
        use std::os::unix::fs::DirBuilderExt;
        std::fs::DirBuilder::new()
            .mode(0o700)
            .create(&root)
            .map_err(|e| e.to_string())?;
        crate::artifacts::persist::create_directory(&root).map_err(|e| e.to_string())?;
        let journal = Self {
            root,
            manifest: Mutex::new(manifest),
        };
        journal.save(&journal.snapshot())?;
        Ok(journal)
    }
    /// Open existing metadata; a missing or malformed manifest fails closed.
    pub fn open(root: &Path) -> Result<Self, String> {
        validate_private(root)?;
        let manifest: RecoveryManifest =
            serde_json::from_slice(&read_manifest(root)?).map_err(|e| e.to_string())?;
        if root.file_name().and_then(|s| s.to_str()) != Some(manifest.run_id.as_str()) {
            return Err("recovery run identity does not match its directory".into());
        }
        manifest.validate()?;
        Ok(Self {
            root: root.to_path_buf(),
            manifest: Mutex::new(manifest),
        })
    }
    /// Protected storage parent; never an artifact path.
    pub fn root(&self) -> &Path {
        &self.root
    }
    /// Read public facts without exposing profile contents.
    pub fn snapshot(&self) -> RecoveryManifest {
        self.manifest
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
    }
    /// Persist startup facts before a scenario can submit an order.
    pub fn set_actors(
        &self,
        identities: BTreeMap<String, String>,
        origin: Option<String>,
    ) -> Result<(), String> {
        if identities.len() != 2
            || !identities.contains_key("alice")
            || !identities.contains_key("bob")
            || identities.values().any(String::is_empty)
            || identities
                .values()
                .collect::<std::collections::BTreeSet<_>>()
                .len()
                != 2
        {
            return Err("recovery requires two distinct public actor identities".into());
        }
        self.update(|manifest| {
            manifest.identities = identities;
            manifest.origin = origin;
        })
    }
    /// Persist the verified cleanup boundary before deleting recovery identities.
    pub fn confirm_resolution(&self) -> Result<(), String> {
        self.update(|manifest| manifest.resolution_verified = true)
    }
    /// Change the completion record only after caller verification.
    pub fn close(&self) -> Result<(), String> {
        self.transition(|manifest| match manifest.state() {
            RecoveryOutcome::AbandonedRegtest { .. } => {
                Err("an abandoned recovery cannot be marked resolved".into())
            }
            _ => {
                manifest.closed = true;
                manifest.outcome = Some(RecoveryOutcome::Resolved);
                Ok(())
            }
        })
    }
    /// Close a pending journal whose regtest stack was destroyed. The caller
    /// proves every safety gate first; this only enforces the transition.
    pub fn abandon(
        &self,
        reason: AbandonReason,
        abandoned_at: chrono::DateTime<chrono::Utc>,
    ) -> Result<(), String> {
        self.transition(|manifest| {
            if !manifest.state().is_pending() {
                return Err(format!(
                    "recovery is already {}; it cannot be abandoned",
                    manifest.state().as_str()
                ));
            }
            manifest.closed = true;
            manifest.outcome = Some(RecoveryOutcome::AbandonedRegtest {
                reason,
                abandoned_at,
            });
            Ok(())
        })
    }
    /// Record the network each actor's node reported for itself.
    pub fn record_node_networks(&self, networks: BTreeMap<String, String>) -> Result<(), String> {
        self.transition(|manifest| {
            let environment = manifest
                .environment
                .as_mut()
                .ok_or("recovery manifest records no environment")?;
            environment.node_networks = networks;
            Ok(())
        })
    }
    fn update(&self, action: impl FnOnce(&mut RecoveryManifest)) -> Result<(), String> {
        self.transition(|manifest| {
            action(manifest);
            Ok(())
        })
    }
    fn transition(
        &self,
        action: impl FnOnce(&mut RecoveryManifest) -> Result<(), String>,
    ) -> Result<(), String> {
        let mut current = self.manifest.lock().unwrap_or_else(|p| p.into_inner());
        let mut next = current.clone();
        action(&mut next)?;
        next.validate()?;
        self.save(&next)?;
        *current = next;
        Ok(())
    }
    fn save(&self, manifest: &RecoveryManifest) -> Result<(), String> {
        let json = serde_json::to_string_pretty(manifest).map_err(|e| e.to_string())?;
        crate::artifacts::persist::atomic_json(&self.root.join("manifest.json"), &json)
            .map_err(|e| e.to_string())
    }
}
impl RecoveryJournal for FileJournal {
    fn events(&self) -> Vec<RecoveryEvent> {
        self.snapshot().events
    }
    fn record(&self, event: RecoveryEvent) -> Result<(), String> {
        let mut current = self.manifest.lock().unwrap_or_else(|p| p.into_inner());
        let mut history = current.clone();
        if let RecoveryEvent::PaymentExpected {
            order_id,
            wallet,
            hash,
            incoming,
            ..
        } = &event
            && history.events.iter().any(|known| matches!(known, RecoveryEvent::PaymentExpected { order_id: order, wallet: actor, hash: known_hash, incoming: direction, .. } if known_hash == hash || (order == order_id && actor == wallet && direction == incoming && !superseded(&history.events, actor, known_hash)))) {
                return Err("a payment was already prepared for this order and actor; refusing replay".into());
        }
        if let RecoveryEvent::PaymentSuperseded {
            order_id,
            wallet,
            hash,
        } = &event
            && !history.events.iter().any(|known| matches!(known, RecoveryEvent::PaymentExpected { order_id: order, wallet: actor, hash: known_hash, incoming: true, .. } if order == order_id && actor == wallet && known_hash == hash))
        {
            return Err("only a prepared incoming payment can be superseded".into());
        }
        history.events.push(event);
        self.save(&history)?;
        *current = history;
        Ok(())
    }
}
/// Whether the expected payment `hash` was withdrawn on purpose, so a
/// replacement for the same order and wallet is a new intent, not a replay.
pub(crate) fn superseded(events: &[RecoveryEvent], wallet: &str, hash: &str) -> bool {
    events.iter().any(|event| {
        matches!(event, RecoveryEvent::PaymentSuperseded { wallet: actor, hash: gone, .. }
            if actor == wallet && gone == hash)
    })
}

fn validate_private(root: &Path) -> Result<(), String> {
    use std::os::unix::fs::MetadataExt;
    if root.canonicalize().map_err(|e| e.to_string())? != root {
        return Err("recovery directory must use a canonical absolute path".into());
    }
    let meta = std::fs::symlink_metadata(root).map_err(|e| e.to_string())?;
    if !meta.is_dir()
        || meta.mode() & 0o077 != 0
        || meta.uid() != rustix::process::getuid().as_raw()
    {
        return Err("recovery directory must be private, owned and not a symlink".into());
    }
    Ok(())
}

fn read_manifest(root: &Path) -> Result<Vec<u8>, String> {
    use rustix::fs::{Mode, OFlags, open, openat};
    use std::{io::Read, os::unix::fs::MetadataExt};
    let flags = OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC | OFlags::NONBLOCK;
    let directory =
        open(root, flags | OFlags::DIRECTORY, Mode::empty()).map_err(|e| e.to_string())?;
    let mut file: std::fs::File = openat(&directory, "manifest.json", flags, Mode::empty())
        .map_err(|e| e.to_string())?
        .into();
    let meta = file.metadata().map_err(|e| e.to_string())?;
    if !meta.is_file()
        || meta.mode() & 0o077 != 0
        || meta.uid() != rustix::process::getuid().as_raw()
        || meta.len() > 1024 * 1024
    {
        return Err("recovery manifest must be a bounded private owned regular file".into());
    }
    let mut bytes = Vec::new();
    (&mut file)
        .take(1024 * 1024 + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() > 1024 * 1024 {
        return Err("recovery manifest exceeds size limit".into());
    }
    Ok(bytes)
}

#[cfg(test)]
#[path = "recovery_store_tests.rs"]
mod tests;
