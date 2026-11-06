//! The CLI runtime's own proof of a stop. Its processes are short-lived
//! children that each run with the actor's `MCLI_DIR`; a marker left by an
//! interrupted coordinator is reconciled only after checking, through
//! `/proc`, that no process of the operator's user may still run with that
//! store (spec §11, runbook: reconcile the owned client processes, never
//! delete the marker to get past a refusal).
//!
//! `/proc` proves a process free of the store only by showing its
//! environment, which it hides even from the process's own user when the
//! process is non-dumpable. For such a process of the operator's user it can
//! go only by the command line: the process counts as using the store when
//! its command line starts with an executable named `mostro-cli`, or cannot
//! be read either. One of another user cannot be a CLI Mortsom started.

use std::ffi::OsStr;
use std::io::ErrorKind;
use std::os::unix::ffi::OsStrExt;
use std::path::Path;

use mortsom_desktop_driver::Platform;
use mortsom_desktop_driver::storage::{CLI_STORE, clear_verified_stop, ensure_stopped};

use rustix::io::Errno;
use rustix::process::{Uid, getuid};

use crate::error::CliError;
use crate::process::EXECUTABLE;

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

/// Whether a process may run with `MCLI_DIR=<mcli>`, as far as `/proc`
/// can tell; see [`any_process_uses`] for what it can and cannot prove.
pub(crate) fn store_in_use(mcli: &Path) -> bool {
    // The real user, not the effective one: a CLI that Mortsom started, in
    // this run or in a killed one, inherited it, and exec changes only the
    // effective and saved users (of a set-user-ID executable).
    any_process_uses(Path::new("/proc"), getuid(), mcli)
}

/// Whether any process in the process table at `proc_root` may run with
/// `MCLI_DIR=<mcli>`.
///
/// A process whose environment can be read uses the store only when the
/// environment holds exactly that variable. One whose environment cannot be
/// read (it made itself non-dumpable, as ssh-agent and gpg-agent do, or its
/// executable is not readable) proves nothing; it counts when its real user
/// is `uid`, the user Mortsom runs the CLI as, and its command line either
/// starts with the [`EXECUTABLE`] or cannot be read either. A process that
/// disappears while it is examined does not count, and a process table that
/// cannot be listed counts as in use.
fn any_process_uses(proc_root: &Path, uid: Uid, mcli: &Path) -> bool {
    let wanted = format!("MCLI_DIR={}", mcli.display());
    let Ok(mut entries) = std::fs::read_dir(proc_root) else {
        // Without a process table nothing can be proved: assume in use.
        return true;
    };
    entries.any(|entry| match entry {
        // An entry that cannot be listed may be the one that uses it.
        Err(_) => true,
        Ok(entry) => is_pid(&entry.file_name()) && uses(&entry.path(), uid, wanted.as_bytes()),
    })
}

fn is_pid(name: &OsStr) -> bool {
    !name.is_empty() && name.as_bytes().iter().all(u8::is_ascii_digit)
}

/// Whether the process at `process` may run with the `wanted` variable.
fn uses(process: &Path, uid: Uid, wanted: &[u8]) -> bool {
    match std::fs::read(process.join("environ")) {
        Ok(environ) => environ.split(|byte| *byte == 0).any(|var| var == wanted),
        Err(error) if gone(&error) => false,
        Err(_) => runs_as(process, uid) && may_be_the_cli(process),
    }
}

/// Whether the process's real user is `uid`. A status that cannot be read
/// belongs to a process that is gone or that another user owns.
fn runs_as(process: &Path, uid: Uid) -> bool {
    std::fs::read_to_string(process.join("status"))
        .ok()
        .and_then(|status| real_uid(&status))
        == Some(uid.as_raw())
}

/// The real user, the first of the four on the status's `Uid:` line.
fn real_uid(status: &str) -> Option<u32> {
    status
        .lines()
        .find_map(|line| line.strip_prefix("Uid:"))?
        .split_whitespace()
        .next()?
        .parse()
        .ok()
}

/// Whether the process's command line starts with the CLI executable, or
/// cannot be read while the process still exists.
fn may_be_the_cli(process: &Path) -> bool {
    match std::fs::read(process.join("cmdline")) {
        Ok(cmdline) => {
            let program = cmdline.split(|byte| *byte == 0).next().unwrap_or_default();
            Path::new(OsStr::from_bytes(program)).file_name() == Some(OsStr::new(EXECUTABLE))
        }
        Err(error) => !gone(&error),
    }
}

/// Whether a read failed because the process no longer exists.
fn gone(error: &std::io::Error) -> bool {
    error.kind() == ErrorKind::NotFound || error.raw_os_error() == Some(Errno::SRCH.raw_os_error())
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
        let mut child = spawn_reader(Path::new("/bin/sh"), &mcli);

        let refused = reconcile(&root);
        let in_use = store_in_use(&mcli);
        child.kill().unwrap();
        child.wait().unwrap();

        assert!(matches!(refused, Err(CliError::Storage(_))), "{refused:?}");
        assert!(in_use);
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

        assert!(!store_in_use(&root.join("mcli")));
        reconcile(&root).unwrap();
    }

    /// A fake process table for [`any_process_uses`]: each process is a
    /// directory, and an unreadable file is a directory under that name
    /// (reading it fails, as an inaccessible `environ` does, but not with
    /// "not found"), so the cases hold whoever runs the tests.
    mod scan {
        use super::super::*;
        use rustix::process::Uid;
        use std::path::PathBuf;

        const OPERATOR: u32 = 1000;
        const STORE: &str = "/state/run/bob/mcli";

        struct Proc(tempfile::TempDir);

        impl Proc {
            fn new() -> Self {
                Self(tempfile::tempdir().unwrap())
            }

            fn process(&self, pid: &str) -> PathBuf {
                let dir = self.0.path().join(pid);
                std::fs::create_dir(&dir).unwrap();
                dir
            }

            fn uses_store(&self) -> bool {
                any_process_uses(self.0.path(), Uid::from_raw(OPERATOR), Path::new(STORE))
            }
        }

        fn environ(process: &Path, variables: &[&str]) {
            write_nul_joined(&process.join("environ"), variables);
        }

        fn cmdline(process: &Path, argv: &[&str]) {
            write_nul_joined(&process.join("cmdline"), argv);
        }

        fn write_nul_joined(path: &Path, items: &[&str]) {
            let mut bytes = Vec::new();
            for item in items {
                bytes.extend_from_slice(item.as_bytes());
                bytes.push(0);
            }
            std::fs::write(path, bytes).unwrap();
        }

        fn status(process: &Path, real_uid: u32) {
            let text = format!(
                "Name:\tx\nUmask:\t0022\nUid:\t{real_uid}\t{real_uid}\t{real_uid}\t{real_uid}\nGid:\t100\t100\t100\t100\n"
            );
            std::fs::write(process.join("status"), text).unwrap();
        }

        fn unreadable(process: &Path, file: &str) {
            std::fs::create_dir(process.join(file)).unwrap();
        }

        const CLI: &[&str] = &[
            "/runs/r1/bundle/mostro-cli",
            "--output",
            "json",
            "order",
            "list",
        ];

        #[test]
        fn a_readable_environment_naming_the_store_uses_it() {
            let table = Proc::new();
            let process = table.process("4242");
            environ(
                &process,
                &["HOME=/state/run/bob/home", "MCLI_DIR=/state/run/bob/mcli"],
            );

            assert!(table.uses_store());
        }

        #[test]
        fn a_readable_environment_naming_another_store_does_not() {
            let table = Proc::new();
            let process = table.process("4242");
            environ(
                &process,
                &[
                    "MCLI_DIR=/state/run/alice/mcli",
                    "OLD_MCLI_DIR=/state/run/bob/mcli",
                ],
            );
            cmdline(&process, CLI);

            assert!(!table.uses_store());
        }

        #[test]
        fn the_operator_s_cli_with_an_unreadable_environment_uses_it() {
            let table = Proc::new();
            let process = table.process("4242");
            unreadable(&process, "environ");
            status(&process, OPERATOR);
            cmdline(&process, CLI);

            assert!(table.uses_store());
        }

        #[test]
        fn the_operator_s_agent_with_an_unreadable_environment_does_not() {
            let table = Proc::new();
            for (pid, argv) in [
                ("100", &["/usr/bin/gpg-agent", "--supervised"][..]),
                (
                    "101",
                    &["/usr/bin/ssh-agent", "-D", "-a", "/run/user/1000/ssh"][..],
                ),
                ("102", &["/bin/sh", "/runs/r1/bundle/mostro-cli"][..]),
                ("103", &["/runs/r1/bundle/mostro-cli-old"][..]),
            ] {
                let process = table.process(pid);
                unreadable(&process, "environ");
                status(&process, OPERATOR);
                cmdline(&process, argv);
            }

            assert!(!table.uses_store());
        }

        #[test]
        fn the_operator_s_process_with_nothing_readable_uses_it() {
            let table = Proc::new();
            let process = table.process("4242");
            unreadable(&process, "environ");
            status(&process, OPERATOR);
            unreadable(&process, "cmdline");

            assert!(table.uses_store());
        }

        #[test]
        fn another_user_s_process_with_an_unreadable_environment_does_not() {
            let table = Proc::new();
            let process = table.process("4242");
            unreadable(&process, "environ");
            status(&process, OPERATOR + 1);
            cmdline(&process, CLI);

            assert!(!table.uses_store());
        }

        #[test]
        fn a_process_gone_before_its_environment_is_read_does_not() {
            let table = Proc::new();
            let process = table.process("4242");
            status(&process, OPERATOR);
            cmdline(&process, CLI);

            assert!(!table.uses_store());
        }

        #[test]
        fn a_process_gone_before_its_status_is_read_does_not() {
            let table = Proc::new();
            let process = table.process("4242");
            unreadable(&process, "environ");
            cmdline(&process, CLI);

            assert!(!table.uses_store());
        }

        #[test]
        fn a_process_gone_before_its_command_line_is_read_does_not() {
            let table = Proc::new();
            let process = table.process("4242");
            unreadable(&process, "environ");
            status(&process, OPERATOR);

            assert!(!table.uses_store());
        }

        #[test]
        fn only_numeric_entries_are_processes() {
            let table = Proc::new();
            for name in ["self", "thread-self", "12a"] {
                let entry = table.process(name);
                environ(&entry, &["MCLI_DIR=/state/run/bob/mcli"]);
                unreadable(&entry, "cmdline");
            }

            assert!(!table.uses_store());
        }

        #[test]
        fn the_real_user_is_the_first_on_the_uid_line() {
            let status = "Name:\tx\nUid:\t1000\t0\t0\t0\nGid:\t1000\t0\t0\t0\n";

            assert_eq!(real_uid(status), Some(1000));
            assert_eq!(real_uid("Name:\tx\n"), None);
            assert_eq!(real_uid("Uid:\t\n"), None);
        }

        #[test]
        fn an_unreadable_process_table_proves_nothing() {
            let table = Proc::new();
            let missing = table.0.path().join("no-proc");

            assert!(any_process_uses(
                &missing,
                Uid::from_raw(OPERATOR),
                Path::new(STORE)
            ));
        }
    }
}
