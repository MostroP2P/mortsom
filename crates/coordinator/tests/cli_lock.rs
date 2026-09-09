//! Mutation commands share an operator lock independently of artifact paths.
#![allow(clippy::unwrap_used)]
use assert_cmd::cargo::cargo_bin_cmd;
use mortsom_coordinator::lock::RunLock;

#[test]
fn busy_host_blocks_mutations_before_loading_config_or_starting_processes() {
    let home = tempfile::tempdir().unwrap();
    let _held = RunLock::acquire_host(home.path()).unwrap();
    for args in [
        vec!["run", "happy_sell", "--app-artifact", "/first/artifact"],
        vec!["run", "happy_sell", "--app-artifact", "/second/artifact"],
        vec!["stack", "up"],
        vec!["stack", "down"],
        vec!["cleanup", "missing-run"],
        vec!["app", "setup"],
        vec!["device", "smoke"],
        vec!["relay", "smoke"],
        vec!["relay", "down", "--all"],
    ] {
        cargo_bin_cmd!("mortsom")
            .env("HOME", home.path())
            .args([
                "--config",
                "/missing/config",
                "--stack-dir",
                "/missing/stack",
            ])
            .args(args)
            .assert()
            .code(3);
    }
    cargo_bin_cmd!("mortsom")
        .env("HOME", home.path())
        .arg("list")
        .assert()
        .success();
}
