//! The CLI runtime's own proof of a stop. Its processes are short-lived
//! children that each run with the actor's `MCLI_DIR`; a marker left by an
//! interrupted coordinator is reconciled only after checking, through
//! `/proc`, that no process of the operator's user that `/proc` shows may
//! still run with that store (spec §11, runbook: reconcile the owned client
//! processes, never delete the marker to get past a refusal).
//!
//! That check needs `/proc` to be the procfs of Mortsom's own PID
//! namespace, listing every process of the operator's user. Mounted with
//! `hidepid=invisible` (while that user is not in its `gid=` group) or
//! `hidepid=ptraceable`, it leaves out every non-dumpable process of that
//! user, so nothing can be proved and the storage stays refused.
//!
//! `/proc` proves a listed process free of the store only by showing its
//! environment, which it hides even from the process's own user when the
//! process is non-dumpable. For such a process of the operator's user (by
//! any of the users its status names or, when `hidepid=noaccess` hides that
//! too, by the owner of its `/proc` directory) it can go only by the
//! command line: the process counts as using the store when its first
//! argument names an executable `mostro-cli`, or when its command line
//! cannot be read either. One of another user cannot be a CLI Mortsom
//! started.

mod procfs;
mod scan;

use std::path::Path;

use mortsom_desktop_driver::Platform;
use mortsom_desktop_driver::storage::{CLI_STORE, clear_verified_stop, ensure_stopped};

use rustix::process::getuid;

use crate::error::CliError;

use scan::any_process_uses;

/// Clears a marker left by an interrupted invocation once no process uses
/// the actor's store; refuses while one may still do, and refuses storage
/// that is not a CLI actor's. Recovery calls it too, before it proves a
/// retained CLI actor inactive.
pub fn reconcile(root: &Path) -> Result<(), CliError> {
    if ensure_stopped(root).is_ok() {
        return Ok(());
    }
    let mcli = root.join(CLI_STORE);
    if let Some(refusal) = store_in_use(&mcli) {
        return Err(CliError::Storage(refusal.into()));
    }
    tracing::info!(root = %root.display(), "no CLI process uses the actor's store; clearing its runtime marker");
    clear_verified_stop(root, Platform::Cli).map_err(|e| CliError::Storage(e.to_string()))
}

/// Why a process may still run with `MCLI_DIR=<mcli>`, as far as `/proc`
/// can tell, or `None` when none can; see [`scan::any_process_uses`] for
/// what a listed process proves.
pub(crate) fn store_in_use(mcli: &Path) -> Option<&'static str> {
    let proc_root = Path::new("/proc");
    if !procfs::lists_every_process(proc_root, std::process::id(), procfs::in_group) {
        return Some(
            "/proc does not list every process of this user (it cannot be read, is mounted with \
             hidepid, or belongs to another PID namespace), so no stop of the actor's CLI can be \
             proved",
        );
    }
    // The real user: a CLI that Mortsom started, in this run or in a killed
    // one, inherited it, and exec changes only the effective and saved
    // users (of a set-user-ID executable). The scan accepts any of the four.
    any_process_uses(proc_root, getuid(), mcli)
        .then_some("a mostro-cli process still uses this actor's store; it must exit first")
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::scan::{gone, may_be_the_cli, runs_as};
    use super::*;
    use mortsom_desktop_driver::storage::{ActiveRuntime, ActorStorage};
    use std::io::ErrorKind;

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
        let mut child = spawn_reader(Path::new("/bin/sh"), &mcli);

        let refused = reconcile(&root);
        let in_use = store_in_use(&mcli);
        child.kill().unwrap();
        child.wait().unwrap();

        assert!(matches!(refused, Err(CliError::Storage(_))), "{refused:?}");
        assert!(in_use.is_some());
        assert!(ensure_stopped(&root).is_err());
    }

    /// On the real process table, a live process of this user whose
    /// executable it may run but not read is non-dumpable: its environment
    /// cannot be read, and that failure does not read as the process being
    /// gone, so it is judged by its user and command line. It is not named
    /// like the CLI, since a hidden CLI would hold every store meanwhile.
    #[test]
    fn a_hidden_environment_of_this_user_is_judged_by_its_command_line() {
        use std::os::unix::fs::PermissionsExt;
        if rustix::process::geteuid().is_root() {
            // Root reads every process's environment.
            return;
        }
        let parent = tempfile::tempdir().unwrap();
        let program = parent.path().join("hidden-sh");
        std::fs::copy("/bin/sh", &program).unwrap();
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o111)).unwrap();
        let mut child = spawn_reader(&program, &parent.path().join("mcli"));
        let process = Path::new("/proc").join(child.id().to_string());

        let environ = std::fs::read(process.join("environ"));
        let operator_s = runs_as(&process, getuid());
        let cli = may_be_the_cli(&process);
        child.kill().unwrap();
        child.wait().unwrap();

        let error = environ.expect_err("a non-dumpable process hides its environment");
        assert!(!gone(&error), "{error}");
        assert!(operator_s);
        assert!(!cli);
    }

    /// Starts `program -c 'read line'` with `MCLI_DIR=<mcli>`. It blocks on
    /// its standard input until it is killed (or the test binary exits),
    /// however long a loaded host takes to scan /proc. A file just written
    /// may still be open in a child another test forked meanwhile, which
    /// makes it busy for a moment.
    fn spawn_reader(program: &Path, mcli: &Path) -> std::process::Child {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        loop {
            let spawned = std::process::Command::new(program)
                .args(["-c", "read line"])
                .stdin(std::process::Stdio::piped())
                .env("MCLI_DIR", mcli)
                .spawn();
            match spawned {
                Err(error)
                    if error.kind() == ErrorKind::ExecutableFileBusy
                        && std::time::Instant::now() < deadline =>
                {
                    std::thread::yield_now();
                }
                other => return other.unwrap(),
            }
        }
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

        assert_eq!(store_in_use(&root.join("mcli")), None);
        reconcile(&root).unwrap();
    }
}
