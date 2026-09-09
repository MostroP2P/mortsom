//! Artifact commands work offline with the default desktop configuration.
#![allow(clippy::unwrap_used)]
use assert_cmd::cargo::cargo_bin_cmd;
use std::path::Path;

fn command(root: &Path) -> assert_cmd::Command {
    let mut command = cargo_bin_cmd!("mortsom");
    command
        .current_dir(root)
        .env_clear()
        .env("HOME", root)
        .env("PATH", "/missing-tools")
        .args(["--stack-dir", root.to_str().unwrap()]);
    command
}

fn fixture(root: &Path) -> String {
    std::fs::create_dir(root.join("config")).unwrap();
    std::fs::write(
        root.join("config/desktop.example.toml"),
        include_str!("../../../config/desktop.example.toml"),
    )
    .unwrap();
    let secret = root
        .join("local/creds/alice/admin.macaroon")
        .display()
        .to_string();
    for run in ["run-a", "run-b"] {
        let profile = if run == "run-a" {
            "mobile-v2"
        } else {
            "mobile-legacy"
        };
        let dir = root.join("artifacts").join(run);
        std::fs::create_dir_all(&dir).unwrap();
        let summary = serde_json::json!({
            "run_id":run,"lane":"local","command":"run","app_profile":profile,"app_version":null,
            "platform":"linux","mainnet_cleared":false,"relay":null,"actors":[],
            "results":[{"name":secret,"app_profile":profile,"app_version":null,
                "outcome":{"outcome":"passed"},"attempts":1,"duration":{"secs":1,"nanos":0}}],
            "checks":{"checks":[]},"cleanup":{"entries":[],"unresolved":[]},"budget":null,
            "farm_teardown":[],"relay_teardown":null,"passed":true
        });
        serde_json::from_value::<mortsom_coordinator::run::RunSummary>(summary.clone()).unwrap();
        std::fs::write(dir.join("summary.json"), summary.to_string()).unwrap();
        std::fs::write(
            dir.join("cleanup.json"),
            r#"{"entries":[],"unresolved":[]}"#,
        )
        .unwrap();
        std::fs::write(
            dir.join("run.log"),
            format!("credential location: {secret}"),
        )
        .unwrap();
    }
    secret
}

#[test]
fn default_desktop_artifact_commands_need_neither_exported_runtime_refs_nor_a_stack() {
    let root = tempfile::tempdir().unwrap();
    let secret = fixture(root.path());
    command(root.path()).arg("accounting").assert().success();
    command(root.path())
        .args(["parity", "run-a", "run-b"])
        .assert()
        .success();
    command(root.path())
        .args(["cleanup", "run-a"])
        .assert()
        .code(1)
        .stdout(predicates::str::contains(
            "desktop recovery is not implemented",
        ));
    command(root.path())
        .args([
            "publish",
            "run-a",
            "--out",
            "published",
            "--screenshots",
            "none",
        ])
        .assert()
        .success();
    for file in ["run.log", "summary.json", "publish.json"] {
        let text = std::fs::read_to_string(root.path().join("published").join(file)).unwrap();
        assert!(
            !text.contains(&secret),
            "credential path leaked through {file}"
        );
    }
    assert!(!root.path().join("ci").exists());
}

#[test]
fn a_missing_custom_publication_secret_fails_before_creating_the_output() {
    let root = tempfile::tempdir().unwrap();
    fixture(root.path());
    std::fs::write(
        root.path().join("config/desktop.example.local.toml"),
        "[wallets.alice]\nmacaroon='env:CUSTOM_MACAROON'\n",
    )
    .unwrap();
    command(root.path())
        .args(["publish", "run-a", "--out", "published"])
        .assert()
        .code(2)
        .stderr(predicates::str::contains("CUSTOM_MACAROON"));
    assert!(!root.path().join("published").exists());
}
