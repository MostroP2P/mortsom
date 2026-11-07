//! A fake process table for [`why_in_use`]: each process is a
//! directory, and an unreadable file is a directory under that name
//! (reading it fails, as an inaccessible `environ` does, but not with
//! "not found"), so the cases hold whoever runs the tests.

use super::*;
use std::os::unix::fs::MetadataExt;
use std::path::PathBuf;

const STORE: &str = "/state/run/bob/mcli";

/// The fake table and its owner, who plays the operator: `/proc` gives a
/// process's directory the process's effective user, so the owner of the
/// fake directories is the user they show.
struct Proc {
    root: tempfile::TempDir,
    operator: u32,
}

impl Proc {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let operator = std::fs::metadata(root.path()).unwrap().uid();
        Self { root, operator }
    }

    /// A user that owns none of the table.
    fn stranger(&self) -> u32 {
        self.operator.wrapping_add(1)
    }

    fn process(&self, pid: &str) -> PathBuf {
        let dir = self.root.path().join(pid);
        std::fs::create_dir(&dir).unwrap();
        dir
    }

    fn uses_store(&self) -> bool {
        self.uses_store_as(self.operator)
    }

    fn uses_store_as(&self, uid: u32) -> bool {
        why_in_use(self.root.path(), Uid::from_raw(uid), Path::new(STORE)).is_some()
    }

    /// A real store directory beside the table's processes, and a second
    /// path to it through a symbolic link.
    fn real_store(&self) -> (PathBuf, PathBuf) {
        let store = self.root.path().join("state/run/bob/mcli");
        std::fs::create_dir_all(&store).unwrap();
        let alias = self.root.path().join("alias");
        std::os::unix::fs::symlink(self.root.path().join("state"), &alias).unwrap();
        (store, alias.join("run/bob/mcli"))
    }

    fn uses(&self, store: &Path) -> bool {
        self.why(store).is_some()
    }

    fn why(&self, store: &Path) -> Option<InUse> {
        why_in_use(self.root.path(), Uid::from_raw(self.operator), store)
    }

    /// The scan of the table for `store` as given, its checks included.
    fn scan_for(&self, store: &Store) -> Option<InUse> {
        scan(self.root.path(), Uid::from_raw(self.operator), store)
    }

    /// A path whose metadata no user can read, root included: it runs
    /// through a symbolic link to itself.
    fn looping_path(&self) -> PathBuf {
        let looped = self.root.path().join("loop");
        std::os::unix::fs::symlink(&looped, &looped).unwrap();
        looped.join("mcli")
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

fn status(process: &Path, uid: u32) {
    status_with(process, [uid; 4], 1);
}

fn status_with_users(process: &Path, users: [u32; 4]) {
    status_with(process, users, 1);
}

/// A status whose `Uid:` line names the real, effective, saved and
/// filesystem users, and that counts the process's threads.
fn status_with(process: &Path, [real, effective, saved, fs]: [u32; 4], threads: u32) {
    status_text(
        process,
        &format!(
            "Uid:\t{real}\t{effective}\t{saved}\t{fs}\nGid:\t100\t100\t100\t100\nThreads:\t{threads}\n"
        ),
    );
}

fn status_text(process: &Path, lines: &str) {
    let text = format!("Name:\tx\nUmask:\t0022\n{lines}");
    std::fs::write(process.join("status"), text).unwrap();
}

/// A thread of `process`, listed under its `task` directory.
fn thread(process: &Path, tid: &str) -> PathBuf {
    let dir = process.join("task").join(tid);
    std::fs::create_dir_all(&dir).unwrap();
    dir
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

/// The state directory may be reached through more than one path, such
/// as a bind mount or a symbolic link: the variable names the store when
/// its path leads to the store's directory.
#[test]
fn a_readable_environment_naming_the_store_by_another_path_uses_it() {
    let table = Proc::new();
    let (store, alias) = table.real_store();
    let process = table.process("4242");
    environ(&process, &[&format!("MCLI_DIR={}", alias.display())]);

    assert!(table.uses(&store));
}

/// A path on an unresponsive mount can block a check of it for as long as
/// the mount hangs: a process naming one counts, by that path, once its
/// check does not complete in time.
#[test]
fn a_path_whose_check_does_not_complete_in_time_counts() {
    let table = Proc::new();
    let (store, _) = table.real_store();
    let process = table.process("4242");
    environ(&process, &["MCLI_DIR=/mnt/hung/mcli"]);
    let unanswered = Store::new(&store).checking_paths_with(|_| Err(Incomplete));

    let in_use = table.scan_for(&unanswered);

    assert_eq!(in_use, Some(InUse::Unchecked("/mnt/hung/mcli".into())));
}

/// Only another absolute path needs a check: the store's own path, a
/// relative one and another variable are told by their bytes.
#[test]
fn only_another_absolute_path_is_checked() {
    let table = Proc::new();
    let (store, _) = table.real_store();
    let process = table.process("4242");
    environ(
        &process,
        &["MCLI_DIR=run/bob/mcli", "OLD_MCLI_DIR=/mnt/hung/mcli"],
    );
    let unanswered = Store::new(&store).checking_paths_with(|_| Err(Incomplete));

    let in_use = table.scan_for(&unanswered);

    assert_eq!(in_use, None);
}

#[test]
fn a_check_answers_within_its_bound_or_is_incomplete() {
    let (release, released) = std::sync::mpsc::channel::<()>();

    let answered = bounded(std::time::Duration::from_secs(5), || 7);
    let unanswered = bounded(std::time::Duration::from_millis(20), move || {
        released.recv().ok()
    });
    // Lets the check left behind end.
    release.send(()).unwrap();

    assert_eq!(answered, Ok(7));
    assert_eq!(unanswered, Err(Incomplete));
}

#[test]
fn a_readable_environment_naming_another_real_directory_does_not() {
    let table = Proc::new();
    let (store, _) = table.real_store();
    let elsewhere = table.root.path().join("state/run/alice/mcli");
    std::fs::create_dir_all(&elsewhere).unwrap();
    let process = table.process("4242");
    environ(
        &process,
        &[
            &format!("MCLI_DIR={}", elsewhere.display()),
            &format!("OLD_MCLI_DIR={}", store.display()),
            "MCLI_DIR=run/bob/mcli",
        ],
    );

    assert!(!table.uses(&store));
}

/// A path whose metadata cannot be read (permission is denied on the way
/// to it, or the read fails) may still lead to the store: a process naming
/// one counts, by that path.
#[test]
fn a_path_that_cannot_be_read_counts() {
    let table = Proc::new();
    let (store, _) = table.real_store();
    let process = table.process("4242");
    environ(&process, &["MCLI_DIR=/mnt/denied/mcli"]);
    let denied = Store::new(&store).checking_paths_with(|_| Ok(Identity::Unverified));

    let in_use = table.scan_for(&denied);

    assert_eq!(in_use, Some(InUse::Unchecked("/mnt/denied/mcli".into())));
}

/// The same through the real check, with a path no user can read.
#[test]
fn a_path_through_a_loop_of_links_counts() {
    let table = Proc::new();
    let (store, _) = table.real_store();
    let looping = table.looping_path();
    let process = table.process("4242");
    environ(&process, &[&format!("MCLI_DIR={}", looping.display())]);

    assert_eq!(table.why(&store), Some(InUse::Unchecked(looping)));
}

/// A path that leads nowhere (nothing is there, or it passes through a
/// file) reaches no directory, so it cannot reach the store.
#[test]
fn a_path_that_leads_nowhere_does_not() {
    let table = Proc::new();
    let (store, _) = table.real_store();
    let file = table.root.path().join("file");
    std::fs::write(&file, "").unwrap();
    for (pid, path) in [
        ("4242", table.root.path().join("missing/mcli")),
        ("4243", file.join("mcli")),
    ] {
        environ(
            &table.process(pid),
            &[&format!("MCLI_DIR={}", path.display())],
        );
    }

    assert_eq!(table.why(&store), None);
}

/// Without the store's own identity there is nothing to compare another
/// path with: a process naming a path that leads to a directory counts.
#[test]
fn a_path_counts_when_the_store_itself_cannot_be_read() {
    let table = Proc::new();
    let (_, alias) = table.real_store();
    let store = table.looping_path();
    let process = table.process("4242");
    environ(&process, &[&format!("MCLI_DIR={}", alias.display())]);

    assert_eq!(table.why(&store), Some(InUse::Unchecked(alias)));
}

/// A store that is not there has no identity either, and a CLI creates its
/// store when it starts.
#[test]
fn a_path_counts_when_the_store_is_not_there() {
    let table = Proc::new();
    let (_, alias) = table.real_store();
    let store = table.root.path().join("state/run/carol/mcli");
    let process = table.process("4242");
    environ(&process, &[&format!("MCLI_DIR={}", alias.display())]);

    assert_eq!(table.why(&store), Some(InUse::Unchecked(alias)));
}

/// Only a path that is not there, or that passes through something that is
/// not a directory, is absent; any other failure to read it proves nothing.
#[test]
fn only_a_path_that_is_not_there_is_absent() {
    for absent in [Errno::NOENT, Errno::NOTDIR] {
        assert_eq!(
            Identity::of(Err(errno(absent))),
            Identity::Absent,
            "{absent:?}"
        );
    }
    for other in [Errno::ACCESS, Errno::PERM, Errno::IO, Errno::LOOP] {
        assert_eq!(
            Identity::of(Err(errno(other))),
            Identity::Unverified,
            "{other:?}"
        );
    }
}

/// A path is bytes: one that is not UTF-8 is matched as it is, not as its
/// lossy rendering.
#[test]
fn a_store_path_that_is_not_utf8_is_matched_by_its_bytes() {
    let table = Proc::new();
    let store = Path::new(OsStr::from_bytes(b"/state/run/b\xffb/mcli"));
    let process = table.process("4242");
    std::fs::write(
        process.join("environ"),
        b"MCLI_DIR=/state/run/b\xffb/mcli\0",
    )
    .unwrap();

    assert!(table.uses(store));
}

#[test]
fn the_operator_s_cli_with_an_unreadable_environment_uses_it() {
    let table = Proc::new();
    let process = table.process("4242");
    unreadable(&process, "environ");
    status(&process, table.operator);
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
        status(&process, table.operator);
        cmdline(&process, argv);
    }

    assert!(!table.uses_store());
}

#[test]
fn the_operator_s_process_with_nothing_readable_uses_it() {
    let table = Proc::new();
    let process = table.process("4242");
    unreadable(&process, "environ");
    status(&process, table.operator);
    unreadable(&process, "cmdline");

    assert!(table.uses_store());
}

#[test]
fn another_user_s_process_with_an_unreadable_environment_does_not() {
    let table = Proc::new();
    let process = table.process("4242");
    unreadable(&process, "environ");
    status(&process, table.stranger());
    cmdline(&process, CLI);

    assert!(!table.uses_store());
}

/// `/proc` mounted with `hidepid=noaccess` hides a non-dumpable process's
/// status as well as its environment, but still gives its directory the
/// process's effective user.
#[test]
fn the_operator_s_cli_whose_status_is_hidden_too_uses_it() {
    let table = Proc::new();
    let process = table.process("4242");
    unreadable(&process, "environ");
    unreadable(&process, "status");
    cmdline(&process, CLI);

    assert!(table.uses_store());
}

#[test]
fn another_user_s_cli_whose_status_is_hidden_does_not() {
    let table = Proc::new();
    let process = table.process("4242");
    unreadable(&process, "environ");
    unreadable(&process, "status");
    cmdline(&process, CLI);

    assert!(!table.uses_store_as(table.stranger()));
}

/// The real user is not the only one that ties a process to the operator:
/// any of the four on the `Uid:` line does.
#[test]
fn the_operator_s_cli_counts_whichever_of_its_users_names_the_operator() {
    for position in 0..4 {
        let table = Proc::new();
        let process = table.process("4242");
        let mut users = [table.stranger(); 4];
        users[position] = table.operator;
        unreadable(&process, "environ");
        status_with_users(&process, users);
        cmdline(&process, CLI);

        assert!(table.uses_store(), "the operator in position {position}");
    }
}

#[test]
fn a_cli_whose_status_names_no_user_counts() {
    for lines in ["", "Uid:\n", "Uid:\tx\t1\t1\t1\n"] {
        let table = Proc::new();
        let process = table.process("4242");
        unreadable(&process, "environ");
        status_text(&process, lines);
        cmdline(&process, CLI);

        assert!(table.uses_store(), "{lines:?}");
    }
}

#[test]
fn a_process_gone_before_its_environment_is_read_does_not() {
    let table = Proc::new();
    let process = table.process("4242");
    status(&process, table.operator);
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
    status(&process, table.operator);

    assert!(!table.uses_store());
}

/// A thread-group leader that exits before its other threads is a zombie
/// whose own environment reads as gone (ESRCH; the fake table can only say
/// "not found") or, on older kernels, as empty, while its threads still
/// run with the store.
#[test]
fn a_leader_that_exited_before_its_threads_is_judged_by_them() {
    for leader_environ in [None, Some(&[][..])] {
        let table = Proc::new();
        let process = table.process("4242");
        if let Some(variables) = leader_environ {
            environ(&process, variables);
        }
        status_with(&process, [table.operator; 4], 2);
        cmdline(&process, &[]);
        thread(&process, "4242");
        environ(&thread(&process, "4243"), &["MCLI_DIR=/state/run/bob/mcli"]);

        assert!(table.uses_store(), "leader environ {leader_environ:?}");
    }
}

#[test]
fn a_hidden_thread_of_an_exited_leader_is_judged_by_its_command_line() {
    let table = Proc::new();
    let process = table.process("4242");
    status_with(&process, [table.operator; 4], 2);
    let live = thread(&process, "4243");
    unreadable(&live, "environ");
    status(&live, table.operator);
    cmdline(&live, CLI);

    assert!(table.uses_store());
}

#[test]
fn an_exited_leader_whose_threads_do_not_use_the_store_does_not() {
    let table = Proc::new();
    let process = table.process("4242");
    status_with(&process, [table.operator; 4], 2);
    thread(&process, "4242");
    environ(
        &thread(&process, "4243"),
        &["MCLI_DIR=/state/run/alice/mcli"],
    );

    assert!(!table.uses_store());
}

/// A single-threaded zombie, or a kernel thread with its empty
/// environment, has no other thread to look at.
#[test]
fn a_process_with_no_other_thread_and_no_environment_does_not() {
    for leader_environ in [None, Some(&[][..])] {
        let table = Proc::new();
        let process = table.process("4242");
        if let Some(variables) = leader_environ {
            environ(&process, variables);
        }
        status_with(&process, [table.operator; 4], 1);
        environ(&thread(&process, "4243"), &["MCLI_DIR=/state/run/bob/mcli"]);

        assert!(!table.uses_store(), "leader environ {leader_environ:?}");
    }
}

/// With several threads and no list of them to read, nothing is proved.
#[test]
fn an_exited_leader_whose_threads_cannot_be_listed_counts() {
    let table = Proc::new();
    let process = table.process("4242");
    status_with(&process, [table.operator; 4], 2);
    std::fs::write(process.join("task"), "").unwrap();

    assert!(table.uses_store());
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
fn a_status_ties_the_process_to_the_operator_by_any_of_its_users() {
    let operator = Uid::from_raw(1000);

    assert!(names_user("Uid:\t1000\t0\t0\t0\n", operator));
    assert!(names_user("Uid:\t0\t0\t0\t1000\n", operator));
    assert!(!names_user("Uid:\t0\t0\t0\t0\n", operator));
    // Without a readable `Uid:` line nothing is proved.
    assert!(names_user("Name:\tx\n", operator));
    assert!(names_user("Uid:\t\n", operator));
    assert!(names_user("Uid:\t0\tx\t0\t0\n", operator));
}

fn errno(errno: Errno) -> std::io::Error {
    std::io::Error::from_raw_os_error(errno.raw_os_error())
}

#[test]
fn an_owner_that_cannot_be_read_counts_unless_the_process_is_gone() {
    let operator = Uid::from_raw(1000);

    assert!(owned_by(Ok(1000), operator));
    assert!(!owned_by(Ok(1001), operator));
    assert!(owned_by(Err(errno(Errno::PERM)), operator));
    assert!(owned_by(Err(errno(Errno::ACCESS)), operator));
    assert!(!owned_by(Err(errno(Errno::NOENT)), operator));
    assert!(!owned_by(Err(errno(Errno::SRCH)), operator));
}

/// ESRCH is what `/proc` answers for a process that exited while its
/// entry was open; the fake table can only produce "not found".
#[test]
fn only_a_missing_entry_or_an_exited_process_reads_as_gone() {
    assert!(gone(&errno(Errno::SRCH)));
    assert!(gone(&errno(Errno::NOENT)));
    for other in [Errno::PERM, Errno::ACCESS, Errno::ISDIR, Errno::IO] {
        assert!(!gone(&errno(other)), "{other:?}");
    }
}

#[test]
fn an_unreadable_process_table_proves_nothing() {
    let table = Proc::new();
    let missing = table.root.path().join("no-proc");

    assert_eq!(
        why_in_use(&missing, Uid::from_raw(table.operator), Path::new(STORE)),
        Some(InUse::Process)
    );
}
