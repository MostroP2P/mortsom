//! A fake process table for [`any_process_uses`]: each process is a
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
        any_process_uses(self.root.path(), Uid::from_raw(uid), Path::new(STORE))
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
    status_with_users(process, [uid; 4]);
}

/// A status whose `Uid:` line names the real, effective, saved and
/// filesystem users.
fn status_with_users(process: &Path, [real, effective, saved, fs]: [u32; 4]) {
    status_text(
        process,
        &format!("Uid:\t{real}\t{effective}\t{saved}\t{fs}\nGid:\t100\t100\t100\t100\n"),
    );
}

fn status_text(process: &Path, lines: &str) {
    let text = format!("Name:\tx\nUmask:\t0022\n{lines}Threads:\t1\n");
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

    assert!(any_process_uses(
        &missing,
        Uid::from_raw(table.operator),
        Path::new(STORE)
    ));
}
