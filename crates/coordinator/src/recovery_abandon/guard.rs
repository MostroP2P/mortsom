//! Stack lifecycle guard: no `stack up` or `stack down` may invalidate the
//! identity a pending recovery journal still depends on.
use std::path::Path;

use super::assess::{StackInspector, StackPresence, gate, presence};
use crate::recovery_store::{AbandonReason, RecoveryManifest};

/// The stack operation about to run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LifecycleOperation {
    /// `stack up`, which always mints a new daemon identity.
    Up {
        /// `--replace`: never permission to destroy recoverable state.
        replace: bool,
    },
    /// `stack down`.
    Down,
}

impl LifecycleOperation {
    fn command(self) -> &'static str {
        match self {
            Self::Up { replace: false } => "stack up",
            Self::Up { replace: true } => "stack up --replace",
            Self::Down => "stack down",
        }
    }
}

/// Refuses `operation` while any pending journal's original stack is still
/// present or cannot be proved gone; returns notes for journals whose stack
/// is provably destroyed.
pub async fn lifecycle_guard(
    operation: LifecycleOperation,
    pending: &[RecoveryManifest],
    selected: &Path,
    inspector: &dyn StackInspector,
) -> Result<Vec<String>, String> {
    let mut notes = Vec::new();
    for manifest in pending {
        let recorded = manifest
            .environment
            .as_ref()
            .and_then(|environment| environment.stack_dir.as_deref())
            .unwrap_or(selected);
        let facts = inspector.facts(recorded, selected).await?;
        let run = &manifest.run_id;
        match presence(&manifest.stack, &facts) {
            StackPresence::Live(_) => {
                return Err(format!(
                    "refusing `mortsom {}`: run {run} retains unresolved recovery state that \
                     belongs to this stack; run `mortsom cleanup {run}` first. Never delete or \
                     edit recovery journals by hand.",
                    operation.command()
                ));
            }
            destroyed @ StackPresence::Destroyed { .. } => {
                notes.push(abandon_hint(manifest, &destroyed).unwrap_or_else(|| {
                    format!(
                        "note: run {run} retains unresolved recovery state whose stack is gone, \
                         and it cannot be proved to be an abandonable Mortsom regtest run"
                    )
                }));
            }
            StackPresence::Unprovable(why) => {
                return Err(format!(
                    "refusing `mortsom {}`: run {run} retains unresolved recovery state and it \
                     cannot be proved that its stack is gone ({why}); resolve it with `mortsom \
                     cleanup {run}` first. Never delete or edit recovery journals by hand.",
                    operation.command()
                ));
            }
        }
    }
    Ok(notes)
}

/// The actionable message naming the abandon command, if and only if every
/// recorded gate passes and the original stack is proved destroyed.
pub fn abandon_hint(manifest: &RecoveryManifest, presence: &StackPresence) -> Option<String> {
    if !matches!(presence, StackPresence::Destroyed { .. })
        || gate(manifest, AbandonReason::StackDestroyed).is_err()
    {
        return None;
    }
    let run = &manifest.run_id;
    Some(format!(
        "run {run} belongs to a destroyed Mortsom regtest stack; cleanup cannot resume it.\n\
         Preserve the evidence and close it as abandoned with:\n  {}",
        super::command_for(run)
    ))
}
