//! `mortsom recovery abandon`: the only terminal path for a recovery journal
//! whose Mortsom-owned regtest stack was destroyed. Every safety gate is
//! proved from recorded and live facts first; evidence is written durably
//! before the journal turns terminal, and retained storage is removed last.
//! Abandonment never verifies an order or a payment and never counts as a
//! clean cleanup.

mod assess;
mod guard;
mod live;

pub use assess::{LiveFacts, StackInspector, StackPresence, gate, presence};
pub use guard::{LifecycleOperation, abandon_hint, lifecycle_guard};
pub use live::{LiveStacks, RealStorage, Removal};

use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use mortsom_common::RunId;
use serde::{Deserialize, Serialize};

use crate::recovery_store::{AbandonReason, FileJournal, RecoveryManifest, RecoveryOutcome};
use crate::stack::StackFingerprint;

/// Evidence file inside the run's artifact directory.
pub const ARTIFACT_FILE: &str = "recovery-abandonment.json";
/// Evidence file inside the protected journal directory.
pub const JOURNAL_FILE: &str = "abandonment.json";
/// Stated in every record so nobody reads abandonment as resolution.
pub const NOT_VERIFIED: &str =
    "order and payment resolution was NOT verified; this run is failed and cannot be accepted";

/// What the operator asked for.
#[derive(Debug, Clone)]
pub struct AbandonRequest {
    /// Journal to close.
    pub run_id: RunId,
    /// Why.
    pub reason: AbandonReason,
    /// The run id typed a second time, as confirmation.
    pub confirm_run: String,
}

/// The sanitized public record of an abandonment. No key, credential,
/// invoice or actor storage ever enters it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AbandonmentEvidence {
    /// Abandoned run.
    pub run_id: RunId,
    /// Why.
    pub reason: AbandonReason,
    /// Identity of the destroyed stack.
    pub original_stack: StackFingerprint,
    /// Whether a different stack was present when the journal was closed.
    pub current_stack_present: bool,
    /// Safety gates that passed, by name.
    pub checks_passed: Vec<String>,
    /// When the record was written.
    pub abandoned_at: DateTime<Utc>,
    /// Always false: abandonment verifies nothing.
    pub resolution_verified: bool,
    /// [`NOT_VERIFIED`].
    pub statement: String,
    /// False in the record prepared before the journal turns terminal, true
    /// once it has. A missing field is read as prepared.
    #[serde(default)]
    pub finalized: bool,
}

/// What a run's artifact directory says about abandonment. Anything but
/// [`Abandonment::None`] makes the run failed and never clean: a prepared
/// record belongs to an abandonment that may still be in progress, and an
/// unreadable one is treated as abandonment rather than ignored.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Abandonment {
    /// No record.
    None,
    /// The record written before the journal turned terminal.
    Prepared(AbandonmentEvidence),
    /// The record written after the journal turned terminal.
    Finalized(AbandonmentEvidence),
    /// A record is present but cannot be read; holds why.
    Unreadable(String),
}

impl Abandonment {
    /// Whether the run must be reported as abandoned.
    pub fn is_abandoned(&self) -> bool {
        !matches!(self, Self::None)
    }

    /// The readable record, prepared or finalized.
    pub fn evidence(&self) -> Option<&AbandonmentEvidence> {
        match self {
            Self::Prepared(evidence) | Self::Finalized(evidence) => Some(evidence),
            Self::None | Self::Unreadable(_) => None,
        }
    }
}

/// What `abandon` did.
#[derive(Debug, Clone)]
pub struct AbandonReport {
    /// The durable record.
    pub evidence: AbandonmentEvidence,
    /// The journal was already abandoned; this call only retried removal.
    pub already_abandoned: bool,
    /// Paths left in place for audit.
    pub preserved: Vec<PathBuf>,
    /// Retained private storage removed by this call.
    pub removed: Vec<PathBuf>,
    /// Why removal stopped, when it did; rerunning the command retries it.
    pub removal_error: Option<String>,
}

impl AbandonReport {
    /// Operator-facing account of everything preserved and removed.
    pub fn render(&self) -> String {
        let e = &self.evidence;
        let state = if self.already_abandoned {
            "was already abandoned"
        } else {
            "abandoned"
        };
        let current = if e.current_stack_present {
            "present"
        } else {
            "not present"
        };
        let mut out = format!("recovery {} {state} ({})\n", e.run_id, e.reason.as_str());
        out.push_str(&format!(
            "  original stack {} is destroyed; a different stack is {current}\n",
            e.original_stack.digest
        ));
        for check in &e.checks_passed {
            out.push_str(&format!("  check passed: {check}\n"));
        }
        for path in &self.preserved {
            out.push_str(&format!("  preserved: {}\n", path.display()));
        }
        for path in &self.removed {
            out.push_str(&format!("  removed:   {}\n", path.display()));
        }
        if self.removed.is_empty() && self.removal_error.is_none() {
            out.push_str("  removed:   nothing (no retained storage remained)\n");
        }
        if let Some(error) = &self.removal_error {
            out.push_str(&format!(
                "  NOT removed: {error}; rerun the same command to retry\n"
            ));
        }
        out.push_str(&format!("result: FAILED run; {NOT_VERIFIED}\n"));
        out
    }
}

/// Removal of the retained client profiles and captured bundle.
pub trait RetainedStorage {
    /// Removes what remains under `root`, naming what it removed even when
    /// it stopped early.
    fn remove(&self, root: &Path) -> Removal;
}

/// Where abandonment reads and writes.
pub struct AbandonPaths<'a> {
    /// The journal's directory.
    pub journal: &'a Path,
    /// The stack directory the operator selected.
    pub selected_stack: &'a Path,
}

/// Validates, records and closes the journal, then removes retained storage.
pub async fn abandon(
    request: &AbandonRequest,
    paths: &AbandonPaths<'_>,
    inspector: &dyn StackInspector,
    storage: &dyn RetainedStorage,
    now: DateTime<Utc>,
) -> Result<AbandonReport, String> {
    if request.confirm_run != request.run_id.as_str() {
        return Err(format!(
            "--confirm-run must repeat the run id exactly ({})",
            request.run_id
        ));
    }
    let journal = FileJournal::open(paths.journal)?;
    let manifest = journal.snapshot();
    let already_abandoned = match manifest.state() {
        RecoveryOutcome::Pending => false,
        RecoveryOutcome::AbandonedRegtest { .. } => true,
        RecoveryOutcome::Resolved => {
            return Err(format!(
                "recovery {} was already resolved; there is nothing to abandon",
                request.run_id
            ));
        }
    };
    let prepared = if already_abandoned {
        read_evidence(&paths.journal.join(JOURNAL_FILE))?
    } else {
        prove(request, &manifest, paths, inspector, now).await?
    };
    let run_artifacts = run_artifacts(&manifest)?;
    if !already_abandoned {
        // Reports fail closed from the prepared public record on: a
        // coordinator killed anywhere after this leaves a run that is
        // never clean, whether its journal is still pending (the operator
        // reruns this command, or cleanup resolves it and discards the
        // record) or already terminal (a rerun finalizes the record).
        write_evidence(&run_artifacts, ARTIFACT_FILE, &prepared)?;
        write_evidence(paths.journal, JOURNAL_FILE, &prepared)?;
        journal.abandon(request.reason, now)?;
    }
    let evidence = AbandonmentEvidence {
        finalized: true,
        ..prepared
    };
    write_evidence(&run_artifacts, ARTIFACT_FILE, &evidence)?;
    let Removal {
        removed,
        error: removal_error,
    } = storage.remove(paths.journal);
    Ok(AbandonReport {
        evidence,
        already_abandoned,
        preserved: vec![
            paths.journal.join("manifest.json"),
            paths.journal.join(JOURNAL_FILE),
            run_artifacts.join(ARTIFACT_FILE),
        ],
        removed,
        removal_error,
    })
}

/// Every gate, recorded and live; the evidence to write when all pass.
async fn prove(
    request: &AbandonRequest,
    manifest: &RecoveryManifest,
    paths: &AbandonPaths<'_>,
    inspector: &dyn StackInspector,
    now: DateTime<Utc>,
) -> Result<AbandonmentEvidence, String> {
    let run = &request.run_id;
    let mut checks = gate(manifest, request.reason)?;
    let dir = recorded_dir(manifest)?;
    let facts = inspector.facts(&dir, paths.selected_stack).await?;
    let current_stack_present = match presence(&manifest.stack, &facts) {
        StackPresence::Destroyed {
            current_stack_present,
        } => current_stack_present,
        StackPresence::Live(place) => {
            return Err(format!(
                "the original stack of run {run} still exists ({place}); resolve it with \
                 `mortsom cleanup {run}` instead"
            ));
        }
        StackPresence::Unprovable(why) => {
            return Err(format!(
                "cannot prove the original stack of run {run} is destroyed: {why}"
            ));
        }
    };
    checks.push("no container, volume or generated state of the original stack remains".into());
    Ok(AbandonmentEvidence {
        run_id: manifest.run_id.clone(),
        reason: request.reason,
        original_stack: manifest.stack.clone(),
        current_stack_present,
        checks_passed: checks,
        abandoned_at: now,
        resolution_verified: false,
        statement: NOT_VERIFIED.to_owned(),
        finalized: false,
    })
}

fn recorded_dir(manifest: &RecoveryManifest) -> Result<PathBuf, String> {
    manifest
        .environment
        .as_ref()
        .and_then(|environment| environment.stack_dir.clone())
        .ok_or_else(|| "the journal records no Mortsom-owned stack directory".to_owned())
}

/// The run's own artifact directory, as the journal recorded it. A journal
/// that recorded none cannot be abandoned: the selected configuration is
/// no proof of where the run's reports live. The directory must exist.
fn run_artifacts(manifest: &RecoveryManifest) -> Result<PathBuf, String> {
    let run = &manifest.run_id;
    let Some(base) = manifest
        .environment
        .as_ref()
        .and_then(|environment| environment.artifacts_dir.as_deref())
    else {
        return Err(format!(
            "recovery {run} predates the recorded artifact directory; it cannot be abandoned"
        ));
    };
    let run_dir = base.join(run.as_str());
    if run_dir.is_dir() {
        return Ok(run_dir);
    }
    Err(format!(
        "the artifact directory of run {run} is missing at {}; restore it so the abandonment \
         is recorded next to the run's reports",
        run_dir.display()
    ))
}

fn write_evidence(dir: &Path, name: &str, evidence: &AbandonmentEvidence) -> Result<(), String> {
    let json = serde_json::to_string_pretty(evidence).map_err(|e| e.to_string())?;
    let path = dir.join(name);
    crate::artifacts::persist::create_directory(dir)
        .and_then(|()| crate::artifacts::persist::atomic_json(&path, &json))
        .map_err(|e| format!("cannot persist {}: {e}", path.display()))
}

fn read_evidence(path: &Path) -> Result<AbandonmentEvidence, String> {
    let text = std::fs::read_to_string(path).map_err(|e| {
        format!(
            "abandoned journal lacks its evidence {}: {e}",
            path.display()
        )
    })?;
    serde_json::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))
}

/// What `run_dir` records about abandonment. Only a missing file means
/// none; every other outcome marks the run abandoned.
pub fn abandonment(run_dir: &Path) -> Abandonment {
    let path = run_dir.join(ARTIFACT_FILE);
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Abandonment::None,
        Err(error) => return Abandonment::Unreadable(format!("{}: {error}", path.display())),
    };
    match serde_json::from_str::<AbandonmentEvidence>(&text) {
        Ok(evidence) if evidence.finalized => Abandonment::Finalized(evidence),
        Ok(evidence) => Abandonment::Prepared(evidence),
        Err(error) => Abandonment::Unreadable(format!("{}: {error}", path.display())),
    }
}

/// The readable abandonment record a run directory holds, if any.
pub fn read_artifact(run_dir: &Path) -> Option<AbandonmentEvidence> {
    abandonment(run_dir).evidence().cloned()
}

/// Removes the prepared record an abandonment interrupted before its journal
/// turned terminal left behind, once that journal is verified resolved
/// instead. A finalized or unreadable record is never removed: next to a
/// resolved journal it is a contradiction to investigate, and the run keeps
/// failing closed until someone does.
pub fn discard_prepared(manifest: &RecoveryManifest) -> Result<(), String> {
    if manifest.state() != RecoveryOutcome::Resolved {
        return Ok(());
    }
    let Some(base) = manifest
        .environment
        .as_ref()
        .and_then(|environment| environment.artifacts_dir.as_deref())
    else {
        return Ok(());
    };
    let run_dir = base.join(manifest.run_id.as_str());
    if !matches!(abandonment(&run_dir), Abandonment::Prepared(_)) {
        return Ok(());
    }
    let path = run_dir.join(ARTIFACT_FILE);
    std::fs::remove_file(&path).map_err(|error| {
        format!(
            "recovery is resolved but its prepared abandonment record {} cannot be removed: {error}",
            path.display()
        )
    })
}

/// The command that closes `run_id`, as printed to operators.
pub fn command_for(run_id: &RunId) -> String {
    format!("mortsom recovery abandon {run_id} --reason stack-destroyed --confirm-run {run_id}")
}

/// A valid finalized evidence document, for report tests elsewhere in the
/// crate.
#[cfg(test)]
pub(crate) fn write_test_artifact(run_dir: &Path) {
    write_test_artifact_with(run_dir, true);
}

/// A record that consumers must still read as abandonment: prepared but
/// never finalized, or unreadable.
#[cfg(test)]
#[allow(clippy::expect_used)]
pub(crate) fn write_incomplete_test_artifacts(run_dir: &Path, corrupt: bool) {
    if corrupt {
        std::fs::create_dir_all(run_dir).expect("run dir");
        std::fs::write(run_dir.join(ARTIFACT_FILE), b"{\"run_id\": \"truncated")
            .expect("evidence file");
    } else {
        write_test_artifact_with(run_dir, false);
    }
}

#[cfg(test)]
#[allow(clippy::expect_used)]
fn write_test_artifact_with(run_dir: &Path, finalized: bool) {
    let evidence = AbandonmentEvidence {
        run_id: RunId::new(),
        reason: AbandonReason::StackDestroyed,
        original_stack: StackFingerprint {
            algorithm: "sha256-mortsom-stack-v1".into(),
            digest: "a".repeat(64),
        },
        current_stack_present: true,
        checks_passed: vec!["journal is pending".into()],
        abandoned_at: Utc::now(),
        resolution_verified: false,
        statement: NOT_VERIFIED.to_owned(),
        finalized,
    };
    std::fs::create_dir_all(run_dir).expect("run dir");
    std::fs::write(
        run_dir.join(ARTIFACT_FILE),
        serde_json::to_string(&evidence).expect("evidence json"),
    )
    .expect("evidence file");
}

#[cfg(test)]
mod tests;
