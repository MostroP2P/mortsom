//! The CLI runtime's own proof of a stop. Its processes are short-lived
//! children that each run with the actor's `MCLI_DIR`; a marker left by an
//! interrupted coordinator is reconciled only after checking that no process
//! on this host still runs with that store (spec §11, runbook: reconcile the
//! owned client processes, never delete the marker to get past a refusal).

use std::path::Path;

use mortsom_desktop_driver::Platform;
use mortsom_desktop_driver::storage::{CLI_STORE, clear_verified_stop, ensure_stopped};

use crate::error::CliError;

/// Clears a marker left by an interrupted invocation once no process uses
/// the actor's store; refuses while one still does, and refuses storage
/// that is not a CLI actor's. Recovery calls it too, before it proves a
/// retained CLI actor inactive.
pub fn reconcile(root: &Path) -> Result<(), CliError> {
    if ensure_stopped(root).is_ok() {
        return Ok(());
    }
    let mcli = root.join(CLI_STORE);
    if store_in_use(&mcli) {
        return Err(CliError::Storage(
            "a mostro-cli process still uses this actor's store; it must exit first".into(),
        ));
    }
    tracing::info!(root = %root.display(), "no CLI process uses the actor's store; clearing its runtime marker");
    clear_verified_stop(root, Platform::Cli).map_err(|e| CliError::Storage(e.to_string()))
}

/// Whether any process this user can see runs with `MCLI_DIR=<mcli>`.
pub(crate) fn store_in_use(mcli: &Path) -> bool {
    let wanted = format!("MCLI_DIR={}", mcli.display());
    let Ok(entries) = std::fs::read_dir("/proc") else {
        // Without a process table nothing can be proved: assume in use.
        return true;
    };
    entries.flatten().any(|entry| {
        let numeric = entry
            .file_name()
            .to_str()
            .is_some_and(|name| name.bytes().all(|b| b.is_ascii_digit()));
        numeric
            && std::fs::read(entry.path().join("environ")).is_ok_and(|environ| {
                environ
                    .split(|byte| *byte == 0)
                    .any(|variable| variable == wanted.as_bytes())
            })
    })
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use mortsom_desktop_driver::storage::{ActiveRuntime, ActorStorage};

    fn storage() -> (tempfile::TempDir, std::path::PathBuf) {
        let parent = tempfile::tempdir().unwrap();
        let root = parent.path().canonicalize().unwrap().join("bob");
        ActorStorage::Fresh(root.clone())
            .prepare(Platform::Cli)
            .unwrap();
        (parent, root)
    }

    #[test]
    fn a_marker_no_process_stands_behind_is_reconciled() {
        let (_parent, root) = storage();
        let _left_behind = ActiveRuntime::begin(&root).unwrap();
        assert!(ensure_stopped(&root).is_err());

        reconcile(&root).unwrap();

        ensure_stopped(&root).unwrap();
    }

    #[test]
    fn a_marker_a_live_process_stands_behind_is_kept() {
        let (_parent, root) = storage();
        let _live = ActiveRuntime::begin(&root).unwrap();
        let mcli = root.join("mcli");
        // Blocks on its standard input until it is killed (or the test
        // binary exits), however long a loaded host takes to scan /proc.
        let mut child = std::process::Command::new("/bin/sh")
            .args(["-c", "read line"])
            .stdin(std::process::Stdio::piped())
            .env("MCLI_DIR", &mcli)
            .spawn()
            .unwrap();

        let refused = reconcile(&root);
        let in_use = store_in_use(&mcli);
        child.kill().unwrap();
        child.wait().unwrap();

        assert!(matches!(refused, Err(CliError::Storage(_))), "{refused:?}");
        assert!(in_use);
        assert!(ensure_stopped(&root).is_err());
    }

    #[test]
    fn a_marker_on_another_platform_s_storage_is_never_cleared() {
        let parent = tempfile::tempdir().unwrap();
        let root = parent.path().canonicalize().unwrap().join("bob");
        ActorStorage::Fresh(root.clone())
            .prepare(Platform::Linux)
            .unwrap();
        let _live_app = ActiveRuntime::begin(&root).unwrap();

        let refused = reconcile(&root);

        assert!(matches!(refused, Err(CliError::Storage(_))), "{refused:?}");
        assert!(ensure_stopped(&root).is_err());
    }

    #[test]
    fn a_store_nobody_runs_with_is_free() {
        let (_parent, root) = storage();

        assert!(!store_in_use(&root.join("mcli")));
        reconcile(&root).unwrap();
    }
}
