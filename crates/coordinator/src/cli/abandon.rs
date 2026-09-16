//! `mortsom recovery abandon` and the recovery guard on stack lifecycle.
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use mortsom_ui_driver::process::TokioRunner;

use crate::doctor::{HostProbe, RealHost};
use crate::recovery_abandon::{
    AbandonPaths, AbandonRequest, LifecycleOperation, LiveStacks, RealStorage, StackInspector,
    abandon, abandon_hint, lifecycle_guard, presence,
};
use crate::recovery_store::{AbandonReason, FileJournal, RecoveryManifest};

fn journal_base() -> Result<PathBuf, String> {
    let home = RealHost
        .env("HOME")
        .ok_or("HOME is required to locate recovery journals")?;
    FileJournal::directory(Path::new(&home))
}

pub(super) async fn abandon_command(
    stack_dir: &Path,
    run: &str,
    reason: AbandonReason,
    confirm_run: &str,
) -> ExitCode {
    let prepared = journal_base().and_then(|base| {
        let run_id: mortsom_common::RunId = run.parse()?;
        let journal = base.join(run_id.as_str());
        if !journal.is_dir() {
            return Err(format!("no recovery journal is retained for run {run_id}"));
        }
        Ok((run_id, journal))
    });
    let (run_id, journal) = match prepared {
        Ok(prepared) => prepared,
        Err(error) => {
            eprintln!("{error}");
            return ExitCode::from(2);
        }
    };
    let request = AbandonRequest {
        run_id,
        reason,
        confirm_run: confirm_run.to_owned(),
    };
    let paths = AbandonPaths {
        journal: &journal,
        selected_stack: stack_dir,
    };
    let inspector = LiveStacks {
        runner: &TokioRunner,
    };
    let now = chrono::Utc::now();
    match abandon(&request, &paths, &inspector, &RealStorage, now).await {
        Ok(report) => {
            print!("{}", report.render());
            if report.removal_error.is_some() {
                ExitCode::from(1)
            } else {
                ExitCode::SUCCESS
            }
        }
        Err(error) => {
            eprintln!("{error}");
            ExitCode::from(1)
        }
    }
}

/// Refuses a stack operation that would invalidate a pending journal's
/// stack; prints a note for each journal whose stack is already gone.
pub(super) async fn guard_stack(
    operation: LifecycleOperation,
    stack_dir: &Path,
) -> Result<(), String> {
    let pending = FileJournal::pending(&journal_base()?)
        .map_err(|error| format!("cannot inspect recovery journals: {error}"))?;
    if pending.is_empty() {
        return Ok(());
    }
    let inspector = LiveStacks {
        runner: &TokioRunner,
    };
    for note in lifecycle_guard(operation, &pending, stack_dir, &inspector).await? {
        eprintln!("{note}");
    }
    Ok(())
}

/// The abandon command, when `manifest`'s stack is provably destroyed and
/// every recorded gate passes; `None` whenever anything is uncertain.
pub(super) async fn destroyed_hint(
    manifest: &RecoveryManifest,
    stack_dir: &Path,
) -> Option<String> {
    let recorded = manifest.environment.as_ref()?.stack_dir.clone()?;
    let inspector = LiveStacks {
        runner: &TokioRunner,
    };
    let facts = inspector.facts(&recorded, stack_dir).await.ok()?;
    abandon_hint(manifest, &presence(&manifest.stack, &facts))
}
