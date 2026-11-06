//! A fake process table for [`any_process_uses`]: each process is a
//! directory, and an unreadable file is a directory under that name
//! (reading it fails, as an inaccessible `environ` does, but not with
//! "not found"), so the cases hold whoever runs the tests.

use super::*;
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
