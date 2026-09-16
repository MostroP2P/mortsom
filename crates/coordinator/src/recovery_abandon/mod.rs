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
pub use live::{LiveStacks, RealStorage};

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
    /// Removes what remains under `root` and names what it removed.
    fn remove(&self, root: &Path) -> Result<Vec<PathBuf>, String>;
}

/// Where abandonment reads and writes.
pub struct AbandonPaths<'a> {
    /// The journal's directory.
    pub journal: &'a Path,
    /// `<artifacts_dir>/<run_id>`.
    pub run_artifacts: &'a Path,
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
    let evidence = if already_abandoned {
        let evidence = read_evidence(&paths.journal.join(JOURNAL_FILE))?;
        // The run directory may have been restored or moved since.
        write_evidence(paths, &evidence)?;
        evidence
    } else {
        let evidence = prove(request, &manifest, paths, inspector, now).await?;
        write_evidence(paths, &evidence)?;
        journal.abandon(request.reason, now)?;
        evidence
    };
    let (removed, removal_error) = match storage.remove(paths.journal) {
        Ok(removed) => (removed, None),
        Err(error) => (Vec::new(), Some(error)),
    };
    Ok(AbandonReport {
        evidence,
        already_abandoned,
        preserved: vec![
            paths.journal.join("manifest.json"),
            paths.journal.join(JOURNAL_FILE),
            paths.run_artifacts.join(ARTIFACT_FILE),
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
    })
}

fn recorded_dir(manifest: &RecoveryManifest) -> Result<PathBuf, String> {
    manifest
        .environment
        .as_ref()
        .and_then(|environment| environment.stack_dir.clone())
        .ok_or_else(|| "the journal records no Mortsom-owned stack directory".to_owned())
}

fn write_evidence(paths: &AbandonPaths<'_>, evidence: &AbandonmentEvidence) -> Result<(), String> {
    let json = serde_json::to_string_pretty(evidence).map_err(|e| e.to_string())?;
    for (dir, name) in [
        (paths.run_artifacts, ARTIFACT_FILE),
        (paths.journal, JOURNAL_FILE),
    ] {
        let path = dir.join(name);
        crate::artifacts::persist::create_directory(dir)
            .and_then(|()| crate::artifacts::persist::atomic_json(&path, &json))
            .map_err(|e| format!("cannot persist {}: {e}", path.display()))?;
    }
    Ok(())
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

/// The abandonment record a run directory holds, if any.
pub fn read_artifact(run_dir: &Path) -> Option<AbandonmentEvidence> {
    let text = std::fs::read_to_string(run_dir.join(ARTIFACT_FILE)).ok()?;
    serde_json::from_str(&text).ok()
}

/// The command that closes `run_id`, as printed to operators.
pub fn command_for(run_id: &RunId) -> String {
    format!("mortsom recovery abandon {run_id} --reason stack-destroyed --confirm-run {run_id}")
}

/// A valid evidence document, for report tests elsewhere in the crate.
#[cfg(test)]
#[allow(clippy::expect_used)]
pub(crate) fn write_test_artifact(run_dir: &Path) {
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
