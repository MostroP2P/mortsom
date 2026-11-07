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
    // Neither the whole URI nor the raw secret it carries may appear.
    let raw = secret.rsplit_once("secret=").unwrap().1;
    assert!(!stdout.contains(secret), "{stdout}");
    assert!(!stdout.contains(raw), "{stdout}");
    assert!(!stdout.contains("secret="), "{stdout}");
    assert!(!stdout.contains("nostr+walletconnect://"), "{stdout}");
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

#[test]
fn web_prebuilt_doctor_does_not_require_build_tools() {
    let output = Command::cargo_bin("mortsom")
        .unwrap()
        .args([
            "--config",
            "/nonexistent/config.toml",
            "doctor",
            "--platform",
            "web",
            "--artifact",
            "/prebuilt/web",
            "--no-network",
            "--json",
        ])
        .env_clear()
        .env("PATH", "/nonexistent")
        .assert()
        .failure();
    let report: serde_json::Value = serde_json::from_slice(&output.get_output().stdout).unwrap();
    let names: Vec<_> = report["checks"]
        .as_array()
        .unwrap()
        .iter()
        .map(|check| check["name"].as_str().unwrap())
        .collect();
    for unwanted in ["Xvfb", "flutter", "cargo"] {
        assert!(!names.contains(&unwanted), "{unwanted}: {names:?}");
    }
    assert!(names.contains(&"chromedriver"));
    assert!(names.contains(&"chromium"));
    assert!(
        !names.contains(&"stack"),
        "offline doctor must not query stack"
    );
}

#[test]
fn doctor_source_overrides_replace_invalid_configured_modes_before_diagnosis() {
    let dir = tempfile::tempdir().unwrap();
    let config = example_config(&dir);
    for configured in [
        "source_dir = '/stale/app'\napp_ref = 'main'",
        "app_ref = 'https://github.com/other/app/pull/1'",
        "source_dir = 'env:UNUSED_SOURCE'\napp_ref = 'env:UNUSED_REF'",
    ] {
        std::fs::write(
            dir.path().join("mortsom.local.toml"),
            format!("[mobile]\n{configured}\n"),
        )
        .unwrap();
        for selection in [["--ref", "main"], ["--source", "/app"]] {
            let output = Command::cargo_bin("mortsom")
                .unwrap()
                .args([
                    "--config",
                    config.to_str().unwrap(),
                    "doctor",
                    "--no-network",
                    "--json",
                ])
                .args(selection)
                .env_clear()
                .env("PATH", "/nonexistent")
                .assert()
                .failure();
            let report: serde_json::Value =
                serde_json::from_slice(&output.get_output().stdout).unwrap();
            for check in report["checks"].as_array().unwrap() {
                let detail = check["detail"].as_str().unwrap();
                assert!(
                    !detail.contains("mutually exclusive")
                        && !detail.contains("invalid app selector"),
                    "{selection:?}: {check}"
                );
                assert!(!check["name"].as_str().unwrap().contains("UNUSED_"));
            }
            assert!(report["checks"].as_array().unwrap().iter().any(|check| {
                check["name"] == "env MORTSOM_ALICE_NWC" && check["status"] == "fail"
            }));
        }
    }
}

/// The names of the checks in a `doctor --json` report.
fn check_names(stdout: &[u8]) -> Vec<String> {
    let report: serde_json::Value = serde_json::from_slice(stdout).unwrap();
    report["checks"]
        .as_array()
        .unwrap()
        .iter()
        .map(|check| check["name"].as_str().unwrap().to_owned())
        .collect()
}

#[test]
fn doctor_with_profile_cli_checks_the_cli_and_not_the_app() {
    let dir = tempfile::tempdir().unwrap();
    let config = example_config(&dir);

    let output = Command::cargo_bin("mortsom")
        .unwrap()
        .args(["--config", config.to_str().unwrap()])
        .args(["doctor", "--profile", "cli", "--no-network", "--json"])
        .env_clear()
        .env("PATH", "/nonexistent")
        .assert()
        .failure();

    let names = check_names(&output.get_output().stdout);
    for wanted in [
        "docker",
        "git",
        "cargo",
        "mostro-cli-source",
        "mostro-cli-commands",
    ] {
        assert!(
            names.iter().any(|name| name == wanted),
            "{wanted}: {names:?}"
        );
    }
    for unwanted in ["flutter", "Xvfb", "client"] {
        assert!(
            !names.iter().any(|name| name == unwanted),
            "{unwanted}: {names:?}"
        );
    }
}

#[test]
fn doctor_refuses_a_platform_with_the_cli_profile_in_the_client_check() {
    let dir = tempfile::tempdir().unwrap();
    let config = example_config(&dir);

    let output = Command::cargo_bin("mortsom")
        .unwrap()
        .args(["--config", config.to_str().unwrap()])
        .args(["doctor", "--profile", "cli", "--platform", "linux"])
        .args(["--no-network", "--json"])
        .env_clear()
        .env("PATH", "/nonexistent")
        .assert()
        .code(1);

    let names = check_names(&output.get_output().stdout);
    assert!(names.iter().any(|name| name == "client"), "{names:?}");
    assert!(!names.iter().any(|name| name == "Xvfb"), "{names:?}");
}

/// `--ref` names a branch or pull request of the profile's repository,
/// whether `--profile` or the configuration (through the environment)
/// selects `cli`: an app pull request is refused before any check runs.
#[test]
fn doctor_reads_the_ref_for_the_cli_profile_s_repository() {
    let dir = tempfile::tempdir().unwrap();
    let config = example_config(&dir);
    let app_pull = "https://github.com/MostroP2P/app/pull/1";
    let flagged: [&str; 2] = ["--profile", "cli"];
    let configured: [&str; 0] = [];
    std::fs::write(
        dir.path().join("mortsom.local.toml"),
        "[mobile]\nprofile = \"${MORTSOM_PROFILE}\"\n",
    )
    .unwrap();
    for (selection, profile) in [(&flagged[..], "mobile-v2"), (&configured[..], "cli")] {
        let output = Command::cargo_bin("mortsom")
            .unwrap()
            .args(["--config", config.to_str().unwrap(), "doctor"])
            .args(selection)
            .args(["--ref", app_pull, "--no-network"])
            .env_clear()
            .env("PATH", "/nonexistent")
            .env("MORTSOM_PROFILE", profile)
            .assert()
            .code(2);

        let stderr = String::from_utf8_lossy(&output.get_output().stderr);
        assert!(
            stderr.contains("--ref: invalid mostro-cli selector"),
            "{selection:?}: {stderr}"
        );
        assert!(output.get_output().stdout.is_empty(), "{selection:?}");
    }
}
