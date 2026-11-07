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

/// `doctor --no-network --json` with `mobile.profile` set in the local
/// overlay of the example configuration, from an empty environment.
fn doctor_with_configured_profile(profile: &str) -> assert_cmd::assert::Assert {
    doctor_with_configured_profile_and_env(profile, &[])
}

/// Like [`doctor_with_configured_profile`], with `env` added to the empty
/// environment.
fn doctor_with_configured_profile_and_env(
    profile: &str,
    env: &[(&str, &str)],
) -> assert_cmd::assert::Assert {
    let dir = tempfile::tempdir().unwrap();
    let config = example_config(&dir);
    std::fs::write(
        dir.path().join("mortsom.local.toml"),
        format!("[mobile]\nprofile = \"{profile}\"\n"),
    )
    .unwrap();
    Command::cargo_bin("mortsom")
        .unwrap()
        .args(["--config", config.to_str().unwrap()])
        .args(["doctor", "--no-network", "--json"])
        .env_clear()
        .env("PATH", "/nonexistent")
        .envs(env.iter().copied())
        .assert()
}

/// The `client` checks of a `doctor --json` report.
fn client_checks(stdout: &[u8]) -> Vec<serde_json::Value> {
    let report: serde_json::Value = serde_json::from_slice(stdout).unwrap();
    report["checks"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|check| check["name"] == "client")
        .cloned()
        .collect()
}

/// Without `--profile`, the banner names the client doctor checks: the
/// configured `cli`, not the default app platform.
#[test]
fn doctor_s_banner_names_the_configured_cli_profile() {
    let output = doctor_with_configured_profile("cli").code(1);

    let stderr = String::from_utf8_lossy(&output.get_output().stderr);
    assert!(stderr.contains("doctor · cli · "), "{stderr}");
    assert!(!stderr.contains("doctor · linux"), "{stderr}");
}

/// A configured reference to an unset name is the one `client` failure,
/// naming the variable; no client's tools are checked, and the banner
/// names the profile as unresolved instead of a platform. (The config
/// section still reports the unset variable as its own check.)
#[test]
fn doctor_with_an_unset_profile_reference_has_one_client_failure_and_checks_no_tools() {
    let output = doctor_with_configured_profile("env:MORTSOM_PROFILE").code(1);

    assert_one_unresolved_client_and_no_tools(&output);
}

/// Set but empty is unset, for the banner as for doctor: CI interpolating
/// an undefined variable gives an empty value, and the banner must not
/// name an app platform doctor is not checking.
#[test]
fn doctor_with_a_profile_reference_set_to_an_empty_value_names_no_platform() {
    let output =
        doctor_with_configured_profile_and_env("env:MORTSOM_PROFILE", &[("MORTSOM_PROFILE", "")])
            .code(1);

    assert_one_unresolved_client_and_no_tools(&output);
    let stderr = String::from_utf8_lossy(&output.get_output().stderr);
    assert!(!stderr.contains("doctor · linux"), "{stderr}");
}

/// One `client` failure naming `MORTSOM_PROFILE`, no client's tools
/// checked, and a banner naming the profile as unresolved.
fn assert_one_unresolved_client_and_no_tools(output: &assert_cmd::assert::Assert) {
    let stdout = &output.get_output().stdout;
    let names = check_names(stdout);
    for unwanted in ["docker", "git", "cargo", "flutter", "Xvfb"] {
        assert!(
            !names.iter().any(|name| name == unwanted),
            "{unwanted}: {names:?}"
        );
    }
    let clients = client_checks(stdout);
    assert_eq!(clients.len(), 1, "{names:?}");
    assert_eq!(clients[0]["status"], "fail", "{}", clients[0]);
    let detail = clients[0]["detail"].as_str().unwrap();
    assert!(detail.contains("MORTSOM_PROFILE"), "{detail}");
    let stderr = String::from_utf8_lossy(&output.get_output().stderr);
    assert!(
        stderr.contains("doctor · unresolved profile (MORTSOM_PROFILE not set) · "),
        "{stderr}"
    );
}
