#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use async_trait::async_trait;
use mortsom_environment::daemon::{DaemonInfo, DaemonProbe, RelayList, RelayProbe};
use mortsom_environment::wallet::{WalletProbe, WalletProbeError};
use nostr_sdk::prelude::PublicKey;

use super::*;
use crate::Config;
use mortsom_environment::WalletConfig;

/// Daemon test identity: nsec and its hex pubkey.
const HEX: &str = "8171eb680049f62ce8747d203f57c3f71de6eea308af6f995363896a1e47cc51";
const ALICE: &str = "nostr+walletconnect://b889ff5b1513b641e2a139f661a661364979c5beee91842f8f0ef42ab558e9d4?relay=wss%3A%2F%2Frelay.damus.io&secret=71a8c14c1407c113601079c4302dab36460f0ccd0ad506f1f2dc73b5100e4f3c";
const BOB: &str = "nostr+walletconnect://b889ff5b1513b641e2a139f661a661364979c5beee91842f8f0ef42ab558e9d4?relay=wss%3A%2F%2Frelay.damus.io&secret=0000000000000000000000000000000000000000000000000000000000000001";

/// Scripted host: which/env/exists lookups and canned command outputs.
#[derive(Default)]
struct FakeHost {
    env: BTreeMap<String, String>,
    tools: BTreeMap<String, PathBuf>,
    existing: BTreeSet<PathBuf>,
    outputs: BTreeMap<String, CommandOutput>,
    invocations: Mutex<Vec<String>>,
}

impl FakeHost {
    fn full() -> Self {
        let mut host = Self::default();
        host.env.insert("MORTSOM_MOSTRO_PUBKEY".into(), HEX.into());
        host.env.insert(
            "MORTSOM_MOSTRO_SETTINGS_FILE".into(),
            "/tmp/settings.toml".into(),
        );
        host.env.insert("MORTSOM_ALICE_NWC".into(), ALICE.into());
        host.env.insert("MORTSOM_BOB_NWC".into(), BOB.into());
        for tool in ["docker", "flutter", "cargo", "git", "Xvfb"] {
            host.tools
                .insert(tool.into(), PathBuf::from(format!("/bin/{tool}")));
        }
        for path in [
            "../mobilev2/pubspec.yaml",
            "../mobilev2/lib/main_mortsom.dart",
            "../mobilev2/scripts/frb-generate.sh",
        ] {
            host.existing.insert(PathBuf::from(path));
        }
        host.outputs
            .insert("docker info --format".into(), ok("29.7.2\n"));
        host.outputs.insert(
            "flutter --version".into(),
            ok("Flutter 3.41.9 • channel stable\n"),
        );
        host
    }
}

fn ok(stdout: &str) -> CommandOutput {
    CommandOutput {
        success: true,
        stdout: stdout.into(),
        stderr: String::new(),
    }
}

#[async_trait]
impl HostProbe for FakeHost {
    fn resolve(&self, path: &Path) -> Option<PathBuf> {
        Some(path.to_path_buf())
    }
    fn env(&self, name: &str) -> Option<String> {
        self.env.get(name).cloned()
    }
    fn which(&self, program: &str) -> Option<PathBuf> {
        self.tools.get(program).cloned()
    }
    fn exists(&self, path: &Path) -> bool {
        self.existing.contains(path)
    }
    fn is_file(&self, path: &Path) -> bool {
        self.existing.contains(path)
    }
    fn is_executable(&self, path: &Path) -> bool {
        self.existing.contains(path)
    }
    async fn run(&self, program: &Path, args: &[&str]) -> Result<CommandOutput, String> {
        let name = program.file_name().unwrap().to_string_lossy().into_owned();
        // Key by program plus the first two bare-word arguments, so
        // `docker info --format ...` keys as `docker info` and
        // `flutter --version` keys as `flutter --version`.
        let words: Vec<&str> = args
            .iter()
            .copied()
            .filter(|a| a.chars().all(|c| c.is_ascii_alphabetic() || c == '-'))
            .take(2)
            .collect();
        let key = format!("{name} {}", words.join(" ")).trim().to_owned();
        self.invocations.lock().unwrap().push(key.clone());
        self.outputs
            .get(&key)
            .cloned()
            .ok_or_else(|| format!("no scripted output for `{key}`"))
    }
}

/// Scripted network.
struct FakeNet {
    port: PortState,
    daemon: DaemonProbe,
    wallets_ok: bool,
    balance_sats: u64,
    inbound_sats: Option<u64>,
    network: Option<String>,
    methods: Vec<String>,
}

impl FakeNet {
    fn healthy() -> Self {
        Self {
            port: PortState::Free,
            daemon: DaemonProbe {
                relays: vec![RelayProbe {
                    configured: "ws://host.docker.internal:7500".into(),
                    dialled: "ws://127.0.0.1:7500".into(),
                    error: None,
                }],
                info: Some(DaemonInfo {
                    pubkey: HEX.into(),
                    created_at: std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap()
                        .as_secs(),
                    tags: [
                        ("mostro_version", "0.18.4"),
                        ("protocol_version", "2"),
                        ("lnd_node_pubkey", "02abc"),
                        ("lnd_version", "0.19"),
                        ("lnd_node_alias", "test"),
                    ]
                    .into_iter()
                    .map(|(k, v)| (k.to_owned(), v.to_owned()))
                    .collect(),
                }),
                relay_list: Some(RelayList {
                    created_at: 1,
                    relays: vec!["ws://host.docker.internal:7500".into()],
                }),
            },
            wallets_ok: true,
            balance_sats: 25_000,
            inbound_sats: None,
            network: Some("mainnet".into()),
            methods: vec!["pay_invoice".into(), "get_balance".into()],
        }
    }
}

#[async_trait]
impl NetProbes for FakeNet {
    async fn local_port(&self, _port: u16) -> PortState {
        self.port
    }
    async fn daemon(&self, _relays: &[String], _pubkey: PublicKey) -> DaemonProbe {
        self.daemon.clone()
    }
    async fn wallet(
        &self,
        label: &str,
        _wallet: &WalletConfig,
    ) -> Result<WalletProbe, WalletProbeError> {
        if self.wallets_ok {
            Ok(WalletProbe {
                inbound_sats: self.inbound_sats,
                label: label.into(),
                alias: Some("testwallet".into()),
                network: self.network.clone(),
                methods: self.methods.clone(),
                balance_sats: self.balance_sats,
            })
        } else {
            Err(WalletProbeError::Timeout {
                label: label.into(),
                operation: "get_info",
                timeout: std::time::Duration::from_secs(1),
            })
        }
    }
}

fn example_config() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let text = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../mortsom.toml.example"
    ))
    .unwrap();
    std::fs::write(dir.path().join("mortsom.toml"), text).unwrap();
    dir
}

fn status_of<'a>(report: &'a DoctorReport, name: &str) -> &'a CheckResult {
    report
        .checks
        .iter()
        .find(|c| c.name == name)
        .unwrap_or_else(|| panic!("missing check {name}: {:#?}", report.checks))
}

/// The offline doctor names every missing secret and tool, and never
/// touches the stack directory or the network.
#[tokio::test]
async fn offline_doctor_reports_every_missing_secret_and_tool() {
    use mortsom_ui_driver::process::tests_support::FakeRunner;
    let dir = example_config();
    let host = FakeHost::default();
    let runner = FakeRunner::default();

    let report = run_desktop_doctor(
        &dir.path().join("mortsom.toml"),
        Path::new("/nonexistent/stack"),
        &DesktopDoctorOptions {
            platform: mortsom_desktop_driver::Platform::Linux,
            artifact: None,
            source: Default::default(),
            no_network: true,
        },
        &host,
        &FakeNet::healthy(),
        &runner,
    )
    .await;

    assert!(!report.is_healthy());
    for var in [
        "MORTSOM_ALICE_NWC",
        "MORTSOM_BOB_NWC",
        "MORTSOM_MOSTRO_PUBKEY",
        "MORTSOM_MOSTRO_SETTINGS_FILE",
    ] {
        assert_eq!(
            status_of(&report, &format!("env {var}")).status,
            CheckStatus::Fail
        );
    }
    for tool in ["docker", "flutter", "cargo", "git", "Xvfb"] {
        assert_eq!(status_of(&report, tool).status, CheckStatus::Fail, "{tool}");
    }
    assert_eq!(status_of(&report, "network").status, CheckStatus::Skip);
    assert!(
        runner.calls().is_empty(),
        "offline doctor never resolves the stack"
    );
    let names: Vec<_> = report.checks.iter().map(|c| c.name.as_str()).collect();
    assert!(!names.contains(&"stack"));
    assert!(!report.render().contains("secret="));
}

/// A configured host passes every offline check the doctor can make
/// without the stack.
#[tokio::test]
async fn offline_doctor_passes_on_a_configured_host() {
    use mortsom_ui_driver::process::tests_support::FakeRunner;
    let dir = example_config();
    let host = FakeHost::full();

    let report = run_desktop_doctor(
        &dir.path().join("mortsom.toml"),
        Path::new("/nonexistent/stack"),
        &DesktopDoctorOptions {
            platform: mortsom_desktop_driver::Platform::Web,
            artifact: Some(PathBuf::from("/prebuilt/web")),
            source: Default::default(),
            no_network: true,
        },
        &host,
        &FakeNet::healthy(),
        &FakeRunner::default(),
    )
    .await;

    for name in ["config", "docker"] {
        assert_eq!(status_of(&report, name).status, CheckStatus::Pass, "{name}");
    }
    assert_eq!(
        status_of(&report, "app-artifact").status,
        CheckStatus::Fail,
        "a prebuilt Web bundle that does not exist is named, not assumed"
    );
    let names: Vec<_> = report.checks.iter().map(|c| c.name.as_str()).collect();
    assert!(
        !names.contains(&"flutter"),
        "a prebuilt artifact needs no build tools"
    );
    assert!(!names.contains(&"Xvfb"), "Web needs no virtual display");
}

#[tokio::test]
async fn missing_config_file_fails_config_and_skips_network() {
    use mortsom_ui_driver::process::tests_support::FakeRunner;
    let report = run_desktop_doctor(
        Path::new("/nonexistent/mortsom.toml"),
        Path::new("/nonexistent/stack"),
        &DesktopDoctorOptions {
            platform: mortsom_desktop_driver::Platform::Linux,
            artifact: None,
            source: Default::default(),
            no_network: false,
        },
        &FakeHost::full(),
        &FakeNet::healthy(),
        &FakeRunner::default(),
    )
    .await;

    assert_eq!(status_of(&report, "config").status, CheckStatus::Fail);
    assert_eq!(status_of(&report, "stack").status, CheckStatus::Fail);
    assert_eq!(status_of(&report, "network").status, CheckStatus::Skip);
    assert!(!report.is_healthy());
}

fn preflight_config() -> (tempfile::TempDir, Config) {
    let dir = example_config();
    let host = FakeHost::full();
    let config =
        Config::load_with(&dir.path().join("mortsom.toml"), |name| host.env(name)).unwrap();
    (dir, config)
}

/// An unreachable relay and a silent wallet are named as such: the daemon
/// was never observed, which the preflight turns into a failure rather
/// than a guess.
#[tokio::test]
async fn daemon_and_wallet_failures_are_explicit() {
    let (_dir, config) = preflight_config();
    let mut net = FakeNet::healthy();
    net.port = PortState::InUse;
    net.wallets_ok = false;
    net.daemon.relays[0].error = Some("connection refused".into());
    net.daemon.info = None;

    let report = desktop_preflight(&config, &net).await.checks;

    assert_eq!(status_of(&report, "relay-port").status, CheckStatus::Pass);
    assert_eq!(
        status_of(&report, "relay ws://host.docker.internal:7500").status,
        CheckStatus::Fail
    );
    assert_eq!(
        status_of(&report, "mostro-daemon").status,
        CheckStatus::Fail
    );
    assert_eq!(status_of(&report, "wallet alice").status, CheckStatus::Fail);
    assert!(
        status_of(&report, "wallet alice")
            .detail
            .contains("timed out")
    );
}

#[tokio::test]
async fn protocol_v1_and_missing_lightning_fail() {
    let (_dir, config) = preflight_config();
    let mut net = FakeNet::healthy();
    let info = net.daemon.info.as_mut().unwrap();
    info.tags.insert("protocol_version".into(), "1".into());
    info.tags.remove("lnd_node_pubkey");

    let report = desktop_preflight(&config, &net).await.checks;

    assert_eq!(
        status_of(&report, "mostro-protocol").status,
        CheckStatus::Fail
    );
    assert_eq!(
        status_of(&report, "mostro-lightning").status,
        CheckStatus::Fail
    );
}

#[tokio::test]
async fn relay_list_without_the_mortsom_relay_fails() {
    let (_dir, config) = preflight_config();
    let mut net = FakeNet::healthy();
    net.daemon.relay_list = Some(RelayList {
        created_at: 1,
        relays: vec!["wss://relay.mostro.network".into()],
    });

    let report = desktop_preflight(&config, &net).await.checks;

    assert_eq!(
        status_of(&report, "mostro-relay-list").status,
        CheckStatus::Fail
    );
}

/// A stale info event is only a warning to the standalone doctor, and a
/// warning is not good enough for a preflight that is about to move sats.
#[tokio::test]
async fn stale_info_event_fails_the_preflight() {
    let (_dir, config) = preflight_config();
    let mut net = FakeNet::healthy();
    net.daemon.info.as_mut().unwrap().created_at = 1;

    let report = desktop_preflight(&config, &net).await.checks;

    assert_eq!(
        status_of(&report, "mostro-daemon").status,
        CheckStatus::Fail
    );
    assert!(
        status_of(&report, "mostro-daemon")
            .detail
            .contains("older than")
    );
}

#[tokio::test]
async fn wallet_liquidity_below_the_run_cap_fails_the_preflight() {
    let (_dir, config) = preflight_config();

    // Example caps: max_order_sats = 5000, max_run_spend_sats = 20000.
    let mut net = FakeNet::healthy();
    net.balance_sats = 7_000;
    let report = desktop_preflight(&config, &net).await.checks;
    assert_eq!(status_of(&report, "wallet alice").status, CheckStatus::Fail);
    assert!(
        status_of(&report, "wallet alice")
            .detail
            .contains("below the run cap (20000 sats)")
    );

    net.balance_sats = 100;
    let report = desktop_preflight(&config, &net).await.checks;
    assert_eq!(status_of(&report, "wallet bob").status, CheckStatus::Fail);
    assert!(!report.is_healthy());
}

/// An LND node with ample outbound but too little inbound passes every
/// balance check and then cannot receive the buyer payout: the preflight
/// must fail it before any sats move.
#[tokio::test]
async fn wallet_inbound_liquidity_below_the_run_cap_fails_the_preflight() {
    let (_dir, config) = preflight_config();

    // Example caps: max_order_sats = 5000, max_run_spend_sats = 20000.
    let mut net = FakeNet::healthy();
    net.inbound_sats = Some(1_000);
    let report = desktop_preflight(&config, &net).await.checks;
    assert_eq!(status_of(&report, "wallet alice").status, CheckStatus::Fail);
    assert!(
        status_of(&report, "wallet alice")
            .detail
            .contains("inbound liquidity below the run cap (20000 sats)")
    );

    net.inbound_sats = Some(25_000);
    let report = desktop_preflight(&config, &net).await.checks;
    assert_eq!(status_of(&report, "wallet alice").status, CheckStatus::Pass);
}

/// The info event is republished on the daemon's own long interval (an hour
/// by default), so its absence during a short probe says nothing about the
/// daemon's health when the daemon is demonstrably publishing: a fresh
/// kind-10002 on the same relay proves it is alive and connected. Only
/// silence on every front is a failure.
#[test]
fn a_missing_info_event_warns_while_the_daemon_is_demonstrably_alive() {
    let mut probe = FakeNet::healthy().daemon;
    let expected = vec!["ws://host.docker.internal:7500".to_owned()];
    probe.info = None;

    let mut report = DoctorReport::default();
    net::evaluate_daemon_with_run_relay(&probe, &expected, None, now(), &mut report);
    assert_eq!(
        status_of(&report, "mostro-daemon").status,
        CheckStatus::Warn,
        "the kind-10002 proves the daemon is there"
    );

    // Nothing at all from the pubkey: that is a real failure.
    probe.relay_list = None;
    let mut report = DoctorReport::default();
    net::evaluate_daemon_with_run_relay(&probe, &expected, None, now(), &mut report);
    assert_eq!(
        status_of(&report, "mostro-daemon").status,
        CheckStatus::Fail
    );
}

/// The probe deadline follows the configured daemon health timeout: a fixed
/// ten seconds asks for events the daemon republishes on a longer cycle.
#[test]
fn the_daemon_probe_deadline_follows_the_configured_timeout() {
    assert_eq!(
        RealNet::with_daemon_timeout(std::time::Duration::from_secs(120)).daemon_timeout(),
        std::time::Duration::from_secs(120)
    );
    // Unconfigured callers keep the documented default.
    assert_eq!(RealNet::default().daemon_timeout(), net::DAEMON_TIMEOUT);
}

/// The run relay exists only while a run is live, so a doctor run between
/// runs must not report its absence as a broken relay: nothing is wrong,
/// there is simply no run. A relay that is *not* the run relay stays a
/// failure, and so does the run relay when something else holds its port.
#[test]
fn the_run_relay_is_not_a_failure_while_no_run_is_live() {
    let mut probe = FakeNet::healthy().daemon;
    probe.relays[0].error = Some("connection refused".into());
    let run_relay = net::RunRelay {
        url: "ws://127.0.0.1:7500".to_owned(),
        port_in_use: false,
    };

    let mut report = DoctorReport::default();
    net::evaluate_daemon_with_run_relay(&probe, &[], Some(&run_relay), now(), &mut report);
    assert_eq!(
        status_of(&report, "relay ws://host.docker.internal:7500").status,
        CheckStatus::Skip,
        "the run relay is Mortsom's own and only lives during a run"
    );

    // Something else is on the port: that unreachable relay is real.
    let busy = net::RunRelay {
        port_in_use: true,
        ..run_relay.clone()
    };
    let mut report = DoctorReport::default();
    net::evaluate_daemon_with_run_relay(&probe, &[], Some(&busy), now(), &mut report);
    assert_eq!(
        status_of(&report, "relay ws://host.docker.internal:7500").status,
        CheckStatus::Fail
    );

    // A relay that is not the run relay is always a failure when it is down.
    let mut elsewhere = FakeNet::healthy().daemon;
    elsewhere.relays[0].configured = "wss://relay.mostro.network".to_owned();
    elsewhere.relays[0].dialled = "wss://relay.mostro.network".to_owned();
    elsewhere.relays[0].error = Some("connection refused".into());
    let mut report = DoctorReport::default();
    net::evaluate_daemon_with_run_relay(&elsewhere, &[], Some(&run_relay), now(), &mut report);
    assert_eq!(
        status_of(&report, "relay wss://relay.mostro.network").status,
        CheckStatus::Fail
    );
}

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

/// With no relay reachable there is nothing to conclude about the daemon,
/// which is what the relay-list check already says. Both must agree.
#[test]
fn the_daemon_check_skips_when_no_relay_is_reachable() {
    let mut probe = FakeNet::healthy().daemon;
    probe.relays[0].error = Some("connection refused".into());
    probe.info = None;
    probe.relay_list = None;

    let mut report = DoctorReport::default();
    net::evaluate_daemon_with_run_relay(&probe, &[], None, now(), &mut report);

    assert_eq!(
        status_of(&report, "mostro-daemon").status,
        CheckStatus::Skip,
        "no relay, no verdict"
    );
    assert_eq!(
        status_of(&report, "mostro-relay-list").status,
        CheckStatus::Skip,
        "the two checks must agree"
    );
}

#[tokio::test]
async fn desktop_offline_doctor_checks_only_its_platform_and_never_calls_stack() {
    use mortsom_desktop_driver::Platform;
    use mortsom_ui_driver::process::tests_support::FakeRunner;
    let host = FakeHost::default();
    let net = FakeNet::healthy();
    let runner = FakeRunner::default();
    for platform in [Platform::Linux, Platform::Web] {
        let report = run_desktop_doctor(
            Path::new("/nonexistent/config.toml"),
            Path::new("/nonexistent/stack"),
            &DesktopDoctorOptions {
                platform,
                artifact: None,
                source: Default::default(),
                no_network: true,
            },
            &host,
            &net,
            &runner,
        )
        .await;
        let names: Vec<_> = report.checks.iter().map(|c| c.name.as_str()).collect();
        assert!(names.contains(&"docker"));
        assert_eq!(names.contains(&"Xvfb"), platform == Platform::Linux);
        assert_eq!(names.contains(&"chromedriver"), platform == Platform::Web);
    }
    assert!(runner.calls().is_empty());
}

#[path = "desktop_preflight_tests.rs"]
mod desktop_preflight_tests;

/// Web doctor of a host whose `chromium-browser` answers `--version` with
/// `output`.
async fn web_doctor_with_browser(output: CommandOutput) -> DoctorReport {
    use mortsom_ui_driver::process::tests_support::FakeRunner;
    let dir = example_config();
    let mut host = FakeHost::full();
    for (tool, path) in [
        ("chromium-browser", "/usr/bin/chromium-browser"),
        ("chromedriver", "/usr/bin/chromedriver"),
    ] {
        host.tools.insert(tool.into(), PathBuf::from(path));
    }
    host.outputs
        .insert("chromium-browser --version".into(), output);
    run_desktop_doctor(
        &dir.path().join("mortsom.toml"),
        Path::new("/nonexistent/stack"),
        &DesktopDoctorOptions {
            platform: mortsom_desktop_driver::Platform::Web,
            artifact: Some(PathBuf::from("/prebuilt/web")),
            source: Default::default(),
            no_network: true,
        },
        &host,
        &FakeNet::healthy(),
        &FakeRunner::default(),
    )
    .await
}

/// Ubuntu's `chromium-browser` package is a script that hands over to the
/// Chromium snap, and refuses to start when the snap is gone. Found on
/// `PATH` it looks like a browser; only running it tells.
#[tokio::test]
async fn a_browser_that_does_not_start_fails_the_web_doctor() {
    let refused = CommandOutput {
        success: false,
        stdout: String::new(),
        stderr:
            "\nCommand '/usr/bin/chromium-browser' requires the chromium snap to be installed.\n"
                .into(),
    };

    let report = web_doctor_with_browser(refused).await;

    let chromium = status_of(&report, "chromium");
    assert_eq!(chromium.status, CheckStatus::Fail);
    assert!(
        chromium.detail.contains("requires the chromium snap"),
        "{}",
        chromium.detail
    );
}

#[tokio::test]
async fn a_browser_that_answers_its_version_passes_the_web_doctor() {
    let report = web_doctor_with_browser(ok("Chromium 141.0.7390.54\n")).await;

    let chromium = status_of(&report, "chromium");
    assert_eq!(chromium.status, CheckStatus::Pass);
    assert!(
        chromium.detail.contains("Chromium 141"),
        "{}",
        chromium.detail
    );
}
