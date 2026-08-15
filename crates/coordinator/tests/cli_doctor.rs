//! End-to-end CLI checks for `mortsom doctor` (no network).

#![allow(clippy::unwrap_used, clippy::expect_used)]

use assert_cmd::Command;

fn example_config(dir: &tempfile::TempDir) -> std::path::PathBuf {
    let text = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../mortsom.toml.example"
    ))
    .unwrap();
    let path = dir.path().join("mortsom.toml");
    std::fs::write(&path, text).unwrap();
    path
}

#[test]
fn doctor_reports_missing_secrets_and_exits_nonzero() {
    let dir = tempfile::tempdir().unwrap();
    let config = example_config(&dir);

    let assert = Command::cargo_bin("mortsom")
        .unwrap()
        .args([
            "--config",
            config.to_str().unwrap(),
            "doctor",
            "--no-network",
        ])
        .env_clear()
        .env("PATH", "/nonexistent")
        .assert()
        .failure();

    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    for var in [
        "MORTSOM_ALICE_NWC",
        "MORTSOM_BOB_NWC",
        "MORTSOM_MOSTRO_PUBKEY",
        "MORTSOM_MOSTRO_SETTINGS_FILE",
    ] {
        assert!(stdout.contains(&format!("[FAIL] env {var}")), "{stdout}");
    }
    assert!(stdout.contains("[FAIL] docker"));
    assert!(stdout.contains("[SKIP] network"));
    assert!(stdout.contains("NOT healthy"));
}

#[test]
fn doctor_json_output_never_contains_the_wallet_secret() {
    let dir = tempfile::tempdir().unwrap();
    let config = example_config(&dir);
    let secret = "nostr+walletconnect://b889ff5b1513b641e2a139f661a661364979c5beee91842f8f0ef42ab558e9d4?relay=wss%3A%2F%2Frelay.damus.io&secret=71a8c14c1407c113601079c4302dab36460f0ccd0ad506f1f2dc73b5100e4f3c";

    let assert = Command::cargo_bin("mortsom")
        .unwrap()
        .args([
            "--config",
            config.to_str().unwrap(),
            "doctor",
            "--no-network",
            "--json",
        ])
        .env_clear()
        .env("PATH", "/nonexistent")
        .env("MORTSOM_ALICE_NWC", secret)
        .env("MORTSOM_BOB_NWC", secret)
        .assert()
        .failure();

    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    assert!(!stdout.contains("secret="), "{stdout}");
    assert!(stdout.contains("\"checks\""));
    assert!(stdout.contains("must be different wallets"));
}

#[test]
fn doctor_without_config_file_fails_cleanly() {
    Command::cargo_bin("mortsom")
        .unwrap()
        .args([
            "--config",
            "/nonexistent/mortsom.toml",
            "doctor",
            "--no-network",
        ])
        .env_clear()
        .assert()
        .failure()
        .stdout(predicates::str::contains("[FAIL] config"));
}
