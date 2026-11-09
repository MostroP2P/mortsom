//! `mortsom list --json`: the machine-readable catalog pull request gates
//! commit and select scenarios from.

#![allow(clippy::unwrap_used)]
use assert_cmd::cargo::cargo_bin_cmd;

fn catalog() -> serde_json::Value {
    let home = tempfile::tempdir().unwrap();
    let out = cargo_bin_cmd!("mortsom")
        .env("HOME", home.path())
        .args(["list", "--json"])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).unwrap()
}

#[test]
fn list_json_prints_the_whole_registry_as_one_document() {
    let c = catalog();

    assert_eq!(c["schema"], 1);
    assert_eq!(c["mortsom_version"], env!("CARGO_PKG_VERSION"));
    let names: Vec<&str> = c["scenarios"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["name"].as_str().unwrap())
        .collect();
    assert!(names.contains(&"happy_sell"), "{names:?}");
    assert!(names.contains(&"maker_bond_abandoned"), "{names:?}");
}

#[test]
fn list_json_flags_the_bond_family_and_only_it() {
    let c = catalog();

    for s in c["scenarios"].as_array().unwrap() {
        let is_bond = s["family"] == "bond";
        assert_eq!(s["needs_bonded_daemon"], is_bond, "{s}");
    }
}

#[test]
fn list_json_is_plain_json_without_terminal_colour() {
    let home = tempfile::tempdir().unwrap();
    let out = cargo_bin_cmd!("mortsom")
        .env("HOME", home.path())
        .env("CLICOLOR_FORCE", "1")
        .args(["list", "--json"])
        .output()
        .unwrap();

    assert!(!out.stdout.contains(&0x1b), "escape codes in --json output");
}
