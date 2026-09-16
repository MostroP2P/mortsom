//! Retry the close-before-discard boundary; contradictory evidence stays intact.
use super::{ARTIFACT_FILE, Abandonment, abandonment};
use crate::recovery_store::{RecoveryManifest, RecoveryOutcome};
use std::path::Path;

/// Reconcile only a valid Prepared record bound to the resolved run and stack.
/// Failed removal or sync must be retried before cleanup reports success.
pub fn discard_prepared(manifest: &RecoveryManifest) -> Result<(), String> {
    reconcile(manifest, |path| {
        std::fs::remove_file(path).map_err(|e| e.to_string())
    })
}

fn reconcile(
    manifest: &RecoveryManifest,
    remove: impl FnOnce(&Path) -> Result<(), String>,
) -> Result<(), String> {
    if manifest.state() != RecoveryOutcome::Resolved {
        return Ok(());
    }
    let Some(base) = manifest
        .environment
        .as_ref()
        .and_then(|e| e.artifacts_dir.as_deref())
    else {
        return Ok(());
    };
    let run_dir = base.join(manifest.run_id.as_str());
    let path = run_dir.join(ARTIFACT_FILE);
    match std::fs::symlink_metadata(&path) {
        Ok(meta) if !meta.is_file() || meta.file_type().is_symlink() => {
            return Err(
                "resolved recovery has unsafe abandonment evidence; preserve it for investigation"
                    .into(),
            );
        }
        Err(error) if error.kind() != std::io::ErrorKind::NotFound => return Err(error.to_string()),
        _ => {}
    }
    match abandonment(&run_dir) {
        Abandonment::None => {}
        Abandonment::Prepared(evidence)
            if evidence.run_id == manifest.run_id
                && evidence.original_stack == manifest.stack
                && !evidence.resolution_verified =>
        {
            remove(&path).map_err(|error| format!("recovery is resolved but its prepared abandonment record cannot be removed: {error}"))?;
        }
        _ => return Err(
            "resolved recovery contradicts its abandonment evidence; preserve it for investigation"
                .into(),
        ),
    }
    // Also retry the directory sync if the previous attempt removed the file
    // but failed to make the deletion durable.
    match std::fs::File::open(&run_dir) {
        Ok(dir) => dir.sync_all().map_err(|error| error.to_string()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.to_string()),
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;
    use crate::recovery_abandon::tests::Fixture;
    use crate::recovery_store::FileJournal;

    #[test]
    fn only_a_resolved_journals_matching_prepared_record_is_removed() {
        let fixture = Fixture::new(|_| {});
        let journal = FileJournal::open(&fixture.journal).unwrap();
        fixture.prepared();
        discard_prepared(&journal.snapshot()).unwrap();
        assert!(abandonment(&fixture.artifacts()).is_abandoned());
        journal.close().unwrap();
        discard_prepared(&journal.snapshot()).unwrap();
        assert_eq!(abandonment(&fixture.artifacts()), Abandonment::None);
        for field in [
            "finalized",
            "run_id",
            "original_stack",
            "resolution_verified",
            "unreadable",
        ] {
            fixture.prepared();
            let path = fixture.artifacts().join(ARTIFACT_FILE);
            let mut value: serde_json::Value =
                serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
            match field {
                "run_id" => value[field] = serde_json::json!(mortsom_common::RunId::new()),
                "original_stack" => value[field]["digest"] = "0".repeat(64).into(),
                _ => value[field] = true.into(),
            }
            let bytes = if field == "unreadable" {
                b"{".to_vec()
            } else {
                serde_json::to_vec(&value).unwrap()
            };
            std::fs::write(&path, &bytes).unwrap();
            assert!(discard_prepared(&journal.snapshot()).is_err(), "{field}");
            assert_eq!(std::fs::read(path).unwrap(), bytes);
        }
    }

    #[test]
    fn removal_failure_after_close_is_reported_and_retryable() {
        let fixture = Fixture::new(|_| {});
        let journal = FileJournal::open(&fixture.journal).unwrap();
        fixture.prepared();
        journal.confirm_resolution().unwrap();
        journal.close().unwrap();
        let error = reconcile(&journal.snapshot(), |_| {
            Err("injected unlink failure".into())
        })
        .unwrap_err();
        assert!(error.contains("injected unlink failure"));
        assert!(abandonment(&fixture.artifacts()).is_abandoned());
        discard_prepared(&journal.snapshot()).unwrap();
        assert!(!abandonment(&fixture.artifacts()).is_abandoned());
        discard_prepared(&journal.snapshot()).unwrap();
    }
}
