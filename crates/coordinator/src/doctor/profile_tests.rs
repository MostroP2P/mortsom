//! How `run_desktop_doctor` routes on the profile: the client's tools, the
//! `client` check, and the CLI or the app checks.

use super::*;
use crate::doctor::cli::fake::{AnsweringCli, executable};
use mortsom_desktop_driver::Platform;
use mortsom_ui_driver::process::tests_support::FakeRunner;

/// Checks only the app runs: its build tools and its platform's.
const APP_ONLY: [&str; 4] = ["flutter", "Xvfb", "dbus-daemon", "chromedriver"];

fn options(profile: Option<&str>, platform: Option<Platform>) -> DesktopDoctorOptions {
    DesktopDoctorOptions {
        platform,
        profile: profile.map(str::to_owned),
        cli: Default::default(),
        artifact: None,
        source: Default::default(),
        no_network: true,
    }
}

/// The offline doctor of the example configuration in `dir`.
async fn doctor(
    dir: &tempfile::TempDir,
    options: &DesktopDoctorOptions,
    host: &FakeHost,
    cli: &AnsweringCli,
) -> DoctorReport {
    let runner = FakeRunner::default();
    let report = run_desktop_doctor(
        &dir.path().join("mortsom.toml"),
        Path::new("/nonexistent/stack"),
        options,
        host,
        &FakeNet::healthy(),
        &runner,
        cli,
    )
    .await;
    assert!(
        runner.calls().is_empty(),
        "offline doctor never calls stack"
    );
    report
}

fn names(report: &DoctorReport) -> Vec<&str> {
    report.checks.iter().map(|c| c.name.as_str()).collect()
}

fn assert_no_app_checks(report: &DoctorReport) {
    let names = names(report);
    for name in APP_ONLY {
        assert!(!names.contains(&name), "{name}: {names:?}");
    }
    assert!(
        !names.iter().any(|name| name.starts_with("app")),
        "{names:?}"
    );
}

/// Writes `mobile.profile = <profile>` into the example's local overlay.
fn configure_profile(dir: &tempfile::TempDir, profile: &str) {
    std::fs::write(
        dir.path().join("mortsom.local.toml"),
        format!("[mobile]\nprofile = \"{profile}\"\n"),
    )
    .unwrap();
}

#[tokio::test]
async fn the_cli_profile_checks_the_cli_s_tools_and_defers_its_build() {
    let dir = example_config();
    let cli = AnsweringCli::new("");

    let report = doctor(&dir, &options(Some("cli"), None), &FakeHost::full(), &cli).await;

    for tool in ["docker", "git", "cargo"] {
        assert_eq!(status_of(&report, tool).status, CheckStatus::Pass, "{tool}");
    }
    assert_eq!(
        status_of(&report, "mostro-cli-source").status,
        CheckStatus::Skip
    );
    assert_eq!(
        status_of(&report, "mostro-cli-commands").status,
        CheckStatus::Skip
    );
    assert!(!names(&report).contains(&"client"));
    assert_no_app_checks(&report);
    assert_eq!(cli.calls(), 0);
}

#[tokio::test]
async fn a_missing_cli_tool_fails_its_check() {
    let dir = example_config();

    let report = doctor(
        &dir,
        &options(Some("cli"), None),
        &FakeHost::default(),
        &AnsweringCli::new(""),
    )
    .await;

    for tool in ["docker", "git", "cargo"] {
        let check = status_of(&report, tool);
        assert_eq!(check.status, CheckStatus::Fail, "{tool}");
        assert_eq!(check.detail, format!("{tool} not found"));
    }
}

/// A prebuilt mostro-cli needs no build tools, and its discovery runs
/// through the injected process.
#[tokio::test]
async fn a_prebuilt_cli_is_discovered_and_needs_only_docker() {
    let dir = example_config();
    let bin = tempfile::tempdir().unwrap();
    let cli = AnsweringCli::discovering(crate::mostro_cli::REQUIRED_COMMANDS);
    let mut options = options(Some("cli"), None);
    options.cli.cli_artifact = Some(executable(bin.path()));

    let report = doctor(&dir, &options, &FakeHost::full(), &cli).await;

    assert_eq!(status_of(&report, "docker").status, CheckStatus::Pass);
    assert_eq!(status_of(&report, "mostro-cli").status, CheckStatus::Pass);
    assert_eq!(
        status_of(&report, "mostro-cli-commands").status,
        CheckStatus::Pass
    );
    let names = names(&report);
    for name in ["git", "cargo", "mostro-cli-source"] {
        assert!(!names.contains(&name), "{name}: {names:?}");
    }
    assert_no_app_checks(&report);
    assert_eq!(cli.calls(), 1);
}

/// `--platform` names an app platform: with the CLI it is a `client`
/// failure, and neither client's tools nor checks are reported.
#[tokio::test]
async fn a_platform_with_the_cli_profile_fails_the_client_check() {
    let dir = example_config();
    let cli = AnsweringCli::new("");

    let report = doctor(
        &dir,
        &options(Some("cli"), Some(Platform::Linux)),
        &FakeHost::full(),
        &cli,
    )
    .await;

    let client = status_of(&report, "client");
    assert_eq!(client.status, CheckStatus::Fail);
    assert!(client.detail.contains("--platform"), "{}", client.detail);
    let names = names(&report);
    assert_eq!(
        names.iter().filter(|name| **name == "client").count(),
        1,
        "{names:?}"
    );
    assert!(
        !names.iter().any(|name| name.starts_with("mostro-cli")),
        "{names:?}"
    );
    for tool in ["docker", "git", "cargo"] {
        assert!(!names.contains(&tool), "{tool}: {names:?}");
    }
    assert_no_app_checks(&report);
    assert!(!report.is_healthy());
    assert_eq!(cli.calls(), 0);
}

#[tokio::test]
async fn a_configured_cli_profile_takes_the_cli_checks() {
    let dir = example_config();
    configure_profile(&dir, "cli");

    let report = doctor(
        &dir,
        &options(None, None),
        &FakeHost::full(),
        &AnsweringCli::new(""),
    )
    .await;

    assert_eq!(status_of(&report, "git").status, CheckStatus::Pass);
    assert_eq!(
        status_of(&report, "mostro-cli-source").status,
        CheckStatus::Skip
    );
    assert_no_app_checks(&report);
}

/// The configured profile is read as the loader reads it: an environment
/// reference names the profile, not an unknown one.
#[tokio::test]
async fn a_configured_profile_from_the_environment_takes_its_checks() {
    let dir = example_config();
    configure_profile(&dir, "env:MORTSOM_PROFILE");
    let mut host = FakeHost::full();
    host.env.insert("MORTSOM_PROFILE".into(), "cli".into());

    let report = doctor(&dir, &options(None, None), &host, &AnsweringCli::new("")).await;

    assert_eq!(status_of(&report, "git").status, CheckStatus::Pass);
    assert_eq!(
        status_of(&report, "mostro-cli-source").status,
        CheckStatus::Skip
    );
    assert!(!names(&report).contains(&"client"));
    assert_no_app_checks(&report);
}

/// `--profile mobile-v2` wins over a configured `cli`, and the app is
/// checked on Linux by default.
#[tokio::test]
async fn the_app_profile_flag_wins_over_a_configured_cli() {
    let dir = example_config();
    configure_profile(&dir, "cli");
    let cli = AnsweringCli::new("");

    let report = doctor(
        &dir,
        &options(Some("mobile-v2"), None),
        &FakeHost::full(),
        &cli,
    )
    .await;

    for name in ["flutter", "Xvfb", "app pubspec.yaml"] {
        assert!(names(&report).contains(&name), "{name}");
    }
    assert!(
        !names(&report)
            .iter()
            .any(|name| name.starts_with("mostro-cli")),
        "{:?}",
        names(&report)
    );
    assert_eq!(cli.calls(), 0);
}

#[tokio::test]
async fn an_unknown_profile_fails_the_client_check() {
    let dir = example_config();

    let report = doctor(
        &dir,
        &options(Some("mobile-v1"), None),
        &FakeHost::full(),
        &AnsweringCli::new(""),
    )
    .await;

    let client = status_of(&report, "client");
    assert_eq!(client.status, CheckStatus::Fail);
    assert!(client.detail.contains("mobile-v1"), "{}", client.detail);
    assert!(!names(&report).contains(&"docker"));
}
