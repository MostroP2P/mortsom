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

use scan::{InUse, PATH_CHECK, why_in_use};

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
        return Err(CliError::Storage(refusal));
    }
    tracing::info!(root = %root.display(), "no CLI process uses the actor's store; clearing its runtime marker");
    clear_verified_stop(root, Platform::Cli).map_err(|e| CliError::Storage(e.to_string()))
}

/// Where the process table is.
const PROC: &str = "/proc";

/// Whether `/proc` can prove a stop at all: it is the procfs of Mortsom's
/// own PID namespace and lists every process of the operator's user.
pub(crate) fn proc_lists_every_process() -> bool {
    procfs::lists_every_process(Path::new(PROC), std::process::id(), procfs::in_group)
}

/// Why a process may still run with `MCLI_DIR=<mcli>`, as far as `/proc`
/// can tell, or `None` when none can; see [`scan::why_in_use`] for what a
/// listed process proves.
pub(crate) fn store_in_use(mcli: &Path) -> Option<String> {
    if !proc_lists_every_process() {
        return Some(
            "/proc does not list every process of this user (it cannot be read, is mounted with \
             hidepid, or belongs to another PID namespace), so no stop of the actor's CLI can be \
             proved"
                .into(),
        );
    }
    // The real user: a CLI that Mortsom started, in this run or in a killed
    // one, inherited it, and exec changes only the effective and saved
    // users (of a set-user-ID executable). The scan accepts any of the four.
    why_in_use(Path::new(PROC), getuid(), mcli).map(refusal)
}

/// The refusal a reason to hold the store in use gives the operator.
fn refusal(in_use: InUse) -> String {
    match in_use {
        InUse::Process => {
            "a mostro-cli process still uses this actor's store; it must exit first".into()
        }
        InUse::Unchecked(path) => format!(
            "a process sets MCLI_DIR to {}, which could not be checked within {PATH_CHECK:?} (an \
             unresponsive mount?), so it may still use this actor's store; that path must answer, \
             or the process exit, first",
            path.display()
        ),
    }
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

    /// Where `/proc` cannot prove a stop (mounted with `hidepid`, or the
    /// procfs of another PID namespace) the storage stays refused instead:
    /// a test that needs a stop proved checks that refusal there, and
    /// returns.
    fn refused_where_proc_proves_nothing(outcome: &Result<(), CliError>, root: &Path) -> bool {
        if proc_lists_every_process() {
            return false;
        }
        assert!(matches!(outcome, Err(CliError::Storage(_))), "{outcome:?}");
        assert!(ensure_stopped(root).is_err());
        true
    }

    #[test]
    fn a_marker_no_process_stands_behind_is_reconciled() {
        let (_parent, root) = storage();
        let _left_behind = ActiveRuntime::begin(&root).unwrap();
        assert!(ensure_stopped(&root).is_err());

        let reconciled = reconcile(&root);

        if refused_where_proc_proves_nothing(&reconciled, &root) {
            return;
        }
        reconciled.unwrap();
        ensure_stopped(&root).unwrap();
    }

    #[test]
    fn a_marker_a_live_process_stands_behind_is_kept() {
        let (_parent, root) = storage();
        let _live = ActiveRuntime::begin(&root).unwrap();
        let mcli = root.join("mcli");
        let mut child = spawn_reader(Path::new("/bin/sh"), &mcli).unwrap();

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
    ///
    /// The premise needs a host that hides the environment and shows the
    /// rest; the test checks nothing where it does not hold: a user that
    /// can read any file (root, `CAP_DAC_OVERRIDE`, `CAP_DAC_READ_SEARCH`),
    /// a temporary directory mounted `noexec`, a process left dumpable or
    /// traceable (`CAP_SYS_PTRACE`, `fs.suid_dumpable=1`), or a `/proc`
    /// mounted with `hidepid=noaccess`, which hides the status too.
    #[test]
    fn a_hidden_environment_of_this_user_is_judged_by_its_command_line() {
        use std::os::unix::fs::PermissionsExt;
        let parent = tempfile::tempdir().unwrap();
        let program = parent.path().join("hidden-sh");
        std::fs::copy("/bin/sh", &program).unwrap();
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o111)).unwrap();
        if std::fs::File::open(&program).is_ok() {
            return;
        }
        let mut child = match spawn_reader(&program, &parent.path().join("mcli")) {
            Err(error) if error.kind() == ErrorKind::PermissionDenied => return,
            spawned => spawned.unwrap(),
        };
        let process = Path::new("/proc").join(child.id().to_string());

        let environ = std::fs::read(process.join("environ"));
        let status = std::fs::read(process.join("status"));
        let operator_s = runs_as(&process, getuid());
        let cli = may_be_the_cli(&process);
        child.kill().unwrap();
        child.wait().unwrap();

        let (Err(error), Ok(_)) = (environ, status) else {
            return;
        };
        assert!(!gone(&error), "{error}");
        assert!(operator_s);
        assert!(!cli);
    }

    /// Starts `program -c 'read line'` with `MCLI_DIR=<mcli>`. It blocks on
    /// its standard input until it is killed (or the test binary exits),
    /// however long a loaded host takes to scan /proc. A file just written
    /// may still be open in a child another test forked meanwhile, which
    /// makes it busy for a moment.
    fn spawn_reader(program: &Path, mcli: &Path) -> std::io::Result<std::process::Child> {
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
                other => return other,
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

    /// The operator must know which path to look at: a hung mount, or the
    /// process that names it.
    #[test]
    fn a_refusal_names_the_path_that_did_not_answer() {
        let refused = refusal(InUse::Unchecked("/mnt/hung/mcli".into()));

        assert!(refused.contains("/mnt/hung/mcli"), "{refused}");
        assert!(refusal(InUse::Process).contains("must exit first"));
    }

    #[test]
    fn a_store_nobody_runs_with_is_free() {
        let (_parent, root) = storage();

        let in_use = store_in_use(&root.join("mcli"));

        // Without a marker there is nothing to reconcile, whatever /proc shows.
        reconcile(&root).unwrap();
        if !proc_lists_every_process() {
            // This host's /proc proves no stop: every store counts as in use.
            assert!(in_use.is_some());
            return;
        }
        assert_eq!(in_use, None);
    }
}
