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
            "profile-b"
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
        .success()
        .stdout(predicates::str::contains("nothing to reopen"));
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

#[test]
fn a_mistyped_run_id_is_refused_with_the_run_it_resembles() {
    // Arrange
    let root = tempfile::tempdir().unwrap();
    fixture(root.path());

    // Act / Assert: a typo of a known run stops before touching the host.
    command(root.path())
        .args(["cleanup", "run-aa"])
        .assert()
        .code(2)
        .stderr(predicates::str::contains(
            "run run-aa is not known on this host",
        ))
        .stderr(predicates::str::contains("mortsom cleanup run-a"))
        .stdout(predicates::str::is_empty());

    // An id nothing resembles is still swept, as a crashed run may have left
    // host resources without an artifact directory.
    command(root.path())
        .args(["cleanup", "never-ran"])
        .assert()
        .code(1)
        .stdout(predicates::str::contains("orders UNKNOWN"));
}

fn journal(root: &Path, regtest: bool) -> (String, std::path::PathBuf) {
    use mortsom_coordinator::recovery_store::{
        FileJournal, RecoveryEnvironment, RecoveryManifest, RecoveryOutcome, WalletBackend,
    };
    let base = FileJournal::directory(root).unwrap();
    let run_id = mortsom_common::RunId::new();
    let environment = regtest.then(|| RecoveryEnvironment {
        lane: mortsom_common::Lane::Local,
        allow_mainnet: false,
        stack_dir: Some(root.canonicalize().unwrap()),
        wallet_backend: WalletBackend::StackLnd,
        node_networks: [("alice", "regtest"), ("bob", "regtest")]
            .map(|(a, n)| (a.to_owned(), n.to_owned()))
            .into(),
        artifacts_dir: None,
    });
    let manifest: RecoveryManifest = serde_json::from_value(serde_json::json!({
        "run_id": run_id,
        "stack": {"algorithm": "sha256-mortsom-stack-v1", "digest": "a".repeat(64)},
        "platform": "linux",
        "bundle": {"algorithm": "test", "digest": "b".repeat(64), "files": 1, "bytes": 1},
        "build": mortsom_coordinator::provenance::BuildProvenance::prebuilt(),
        "executable": "mostro", "identities": {}, "origin": null, "events": [],
        "closed": false, "resolution_verified": false,
        "outcome": RecoveryOutcome::Pending, "environment": environment,
    }))
    .unwrap();
    let journal = FileJournal::create(&base, manifest).unwrap();
    (run_id.as_str().to_owned(), journal.root().to_path_buf())
}

fn still_pending(journal: &Path) -> bool {
    mortsom_coordinator::recovery_store::FileJournal::open(journal)
        .unwrap()
        .snapshot()
        .state()
        .is_pending()
}

#[test]
fn recovery_abandon_refuses_without_a_journal_confirmation_or_provable_facts() {
    let root = tempfile::tempdir().unwrap();
    fixture(root.path());
    let abandon = |run: &str, confirm: &str| {
        let mut command = command(root.path());
        command.args([
            "recovery",
            "abandon",
            run,
            "--reason",
            "stack-destroyed",
            "--confirm-run",
            confirm,
        ]);
        command
    };

    abandon("run-a", "run-a")
        .assert()
        .code(2)
        .stderr(predicates::str::contains("no recovery journal"));

    let (legacy, legacy_root) = journal(root.path(), false);
    abandon(&legacy, "run-a")
        .assert()
        .code(1)
        .stderr(predicates::str::contains("--confirm-run"));
    abandon(&legacy, &legacy)
        .assert()
        .code(1)
        .stderr(predicates::str::contains("cannot be abandoned"));
    assert!(still_pending(&legacy_root));

    // Docker is unreachable here: the stack facts cannot be read, so the
    // journal stays open.
    let (regtest, regtest_root) = journal(root.path(), true);
    abandon(&regtest, &regtest).assert().code(1);
    assert!(still_pending(&regtest_root));
    assert!(regtest_root.join("abandonment.json").metadata().is_err());
}

#[test]
fn stack_down_fails_closed_while_a_pending_journal_cannot_be_checked() {
    let root = tempfile::tempdir().unwrap();
    fixture(root.path());
    std::fs::write(root.path().join("compose.yml"), "").unwrap();
    let (_run, journal_root) = journal(root.path(), true);

    command(root.path())
        .args(["stack", "down"])
        .assert()
        .code(2)
        .stdout(predicates::str::is_empty());
    assert!(still_pending(&journal_root));
}
