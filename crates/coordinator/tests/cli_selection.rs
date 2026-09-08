//! Unsupported desktop batches fail before stack access or app generation.
#![allow(clippy::unwrap_used)]
use assert_cmd::cargo::cargo_bin_cmd;

#[test]
fn desktop_batches_are_rejected_before_loading_any_environment() {
    for platform in ["linux", "web"] {
        for selection in [
            vec!["happy_sell", "cancel_pending"],
            vec!["--tag", "regression"],
        ] {
            let root = tempfile::tempdir().unwrap();
            let mut command = cargo_bin_cmd!("mortsom");
            let output = command
                .current_dir(root.path())
                .env_clear()
                .env("HOME", root.path())
                .env("PATH", "/missing-tools")
                .args(["run", "--platform", platform])
                .args(selection)
                .assert()
                .code(2)
                .get_output()
                .clone();
            assert!(
                String::from_utf8_lossy(&output.stderr)
                    .contains("desktop runs require exactly one scenario")
            );
            assert!(!root.path().join("artifacts").exists());
        }
    }
}
