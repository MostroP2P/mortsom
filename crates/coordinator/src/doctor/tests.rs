#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use async_trait::async_trait;
use mortsom_common::Secret;
use mortsom_environment::daemon::{DaemonInfo, DaemonProbe, RelayList, RelayProbe};
use mortsom_environment::wallet::{WalletProbe, WalletProbeError};
use nostr_sdk::prelude::PublicKey;

use super::*;

/// Daemon test identity: nsec and its hex pubkey.
const NSEC: &str = "nsec13as48eum93hkg7plv526r9gjpa0uc52zysqm93pmnkca9e69x6tsdjmdxd";
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
        host.env.insert("ANDROID_HOME".into(), "/sdk".into());
        host.env.insert("MORTSOM_MOSTRO_PUBKEY".into(), HEX.into());
        host.env.insert(
            "MORTSOM_MOSTRO_SETTINGS_FILE".into(),
            "/tmp/settings.toml".into(),
        );
        host.env.insert("MORTSOM_ALICE_NWC".into(), ALICE.into());
        host.env.insert("MORTSOM_BOB_NWC".into(), BOB.into());
        for tool in ["docker", "appium", "java", "flutter"] {
            host.tools
                .insert(tool.into(), PathBuf::from(format!("/bin/{tool}")));
        }
        for path in [
            "/sdk",
            "/sdk/emulator/emulator",
            "/sdk/platform-tools/adb",
            "/sdk/cmdline-tools/latest/bin/avdmanager",
            "/sdk/system-images/android-35/google_apis/x86_64",
            "../mobile/pubspec.yaml",
        ] {
            host.existing.insert(PathBuf::from(path));
        }
        host.outputs
            .insert("docker info --format".into(), ok("29.7.2\n"));
        host.outputs
            .insert("appium --version".into(), ok("2.11.0\n"));
        host.outputs.insert(
            "appium driver list".into(),
            ok("{\"uiautomator2\":{\"version\":\"3.0.0\"}}"),
        );
        host.outputs.insert(
            "java -version".into(),
            CommandOutput {
                success: true,
                stdout: String::new(),
                stderr: "openjdk version \"17.0.19\"\n".into(),
            },
        );
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
    fn env(&self, name: &str) -> Option<String> {
        self.env.get(name).cloned()
    }
    fn which(&self, program: &str) -> Option<PathBuf> {
        self.tools.get(program).cloned()
    }
    fn exists(&self, path: &Path) -> bool {
        self.existing.contains(path)
    }
    async fn run(&self, program: &Path, args: &[&str]) -> Result<CommandOutput, String> {
        let name = program.file_name().unwrap().to_string_lossy().into_owned();
        // Key by program plus the first two bare-word arguments, so
        // `docker info --format ...` keys as `docker info` and
        // `appium --version` keys as `appium --version`.
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
}

impl FakeNet {
    fn healthy() -> Self {
        Self {
            port: PortState::Free,
            daemon: DaemonProbe {
                relays: vec![RelayProbe {
                    configured: "ws://host.docker.internal:7000".into(),
                    dialled: "ws://127.0.0.1:7000".into(),
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
                    relays: vec!["ws://host.docker.internal:7000".into()],
                }),
            },
            wallets_ok: true,
            balance_sats: 25_000,
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
    async fn wallet(&self, label: &str, _uri: &Secret) -> Result<WalletProbe, WalletProbeError> {
        if self.wallets_ok {
            Ok(WalletProbe {
                label: label.into(),
                alias: Some("testwallet".into()),
                network: Some("mainnet".into()),
                methods: vec!["pay_invoice".into(), "get_balance".into()],
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

/// Writes a healthy daemon settings file into `dir` and points the fake
/// host's env and filesystem at it.
fn with_settings(host: &mut FakeHost, dir: &tempfile::TempDir) {
    let path = dir.path().join("settings.toml");
    std::fs::write(
        &path,
        format!(
            "[nostr]\nnsec_privkey = '{NSEC}'\nrelays = ['ws://host.docker.internal:7000']\n[mostro]\nexpiration_seconds = 900\nexpiration_hours = 24\n[lightning]\nhold_invoice_expiration_window = 300\ninvoice_expiration_window = 3600\n"
        ),
    )
    .unwrap();
    let text = path.to_string_lossy().into_owned();
    host.env.insert("MORTSOM_MOSTRO_SETTINGS_FILE".into(), text);
    host.existing.insert(path);
}

fn status_of<'a>(report: &'a DoctorReport, name: &str) -> &'a CheckResult {
    report
        .checks
        .iter()
        .find(|c| c.name == name)
        .unwrap_or_else(|| panic!("missing check {name}: {:#?}", report.checks))
}

#[tokio::test]
async fn healthy_host_and_network_produce_a_healthy_report() {
    let dir = example_config();
    let mut host = FakeHost::full();
    with_settings(&mut host, &dir);

    let report = run_doctor(
        &dir.path().join("mortsom.toml"),
        &DoctorOptions::default(),
        &host,
        &FakeNet::healthy(),
    )
    .await;

    assert!(report.is_healthy(), "{}", report.render());
    for name in [
        "config",
        "docker",
        "android-sdk",
        "emulator",
        "adb",
        "avdmanager",
        "system-image",
        "appium",
        "uiautomator2",
        "java",
        "flutter",
        "mobile-app",
        "daemon-settings",
        "daemon-pubkey",
        "daemon-relays",
        "daemon-transport",
        "relay-port",
        "mostro-daemon",
        "mostro-relay-list",
        "mostro-protocol",
        "mostro-lightning",
        "wallet alice",
        "wallet bob",
    ] {
        assert_eq!(status_of(&report, name).status, CheckStatus::Pass, "{name}");
    }
    let text = report.render();
    assert!(!text.contains("secret="), "report leaked an NWC secret");
    assert!(text.contains("healthy"));
}

#[tokio::test]
async fn every_missing_secret_and_tool_is_reported() {
    let dir = example_config();
    let host = FakeHost::default();

    let report = run_doctor(
        &dir.path().join("mortsom.toml"),
        &DoctorOptions::default(),
        &host,
        &FakeNet::healthy(),
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
    for tool in [
        "docker",
        "android-sdk",
        "emulator",
        "adb",
        "avdmanager",
        "appium",
        "java",
        "flutter",
    ] {
        assert_eq!(status_of(&report, tool).status, CheckStatus::Fail, "{tool}");
    }
    assert_eq!(status_of(&report, "system-image").status, CheckStatus::Skip);
    assert_eq!(status_of(&report, "wallet alice").status, CheckStatus::Skip);
    assert_eq!(
        status_of(&report, "mostro-daemon").status,
        CheckStatus::Skip
    );
}

#[tokio::test]
async fn missing_settings_file_and_wrong_identity_are_reported() {
    let dir = example_config();
    let mut host = FakeHost::full();
    // A settings path that the host reports as existing but that was never
    // created, so the read fails; a real /tmp/settings.toml on the machine
    // running the tests must not change the outcome.
    let unreadable = dir.path().join("never-created-settings.toml");
    host.env.insert(
        "MORTSOM_MOSTRO_SETTINGS_FILE".into(),
        unreadable.to_string_lossy().into_owned(),
    );
    host.existing.insert(unreadable);
    let report = run_doctor(
        &dir.path().join("mortsom.toml"),
        &DoctorOptions { no_network: true },
        &host,
        &FakeNet::healthy(),
    )
    .await;
    assert_eq!(
        status_of(&report, "daemon-settings").status,
        CheckStatus::Fail
    );

    let mut host = FakeHost::full();
    with_settings(&mut host, &dir);
    host.env.insert(
        "MORTSOM_MOSTRO_PUBKEY".into(),
        "b889ff5b1513b641e2a139f661a661364979c5beee91842f8f0ef42ab558e9d4".into(),
    );
    let report = run_doctor(
        &dir.path().join("mortsom.toml"),
        &DoctorOptions { no_network: true },
        &host,
        &FakeNet::healthy(),
    )
    .await;
    assert_eq!(
        status_of(&report, "daemon-pubkey").status,
        CheckStatus::Fail
    );
}

#[tokio::test]
async fn no_network_skips_every_network_check() {
    let dir = example_config();
    let host = FakeHost::full();

    let report = run_doctor(
        &dir.path().join("mortsom.toml"),
        &DoctorOptions { no_network: true },
        &host,
        &FakeNet::healthy(),
    )
    .await;

    assert_eq!(status_of(&report, "network").status, CheckStatus::Skip);
    assert!(report.checks.iter().all(|c| !c.name.starts_with("wallet")));
    assert!(
        host.invocations
            .lock()
            .unwrap()
            .iter()
            .any(|i| i.starts_with("docker"))
    );
}

#[tokio::test]
async fn daemon_and_wallet_failures_are_explicit() {
    let dir = example_config();
    let host = FakeHost::full();
    let mut net = FakeNet::healthy();
    net.port = PortState::InUse;
    net.wallets_ok = false;
    net.daemon.relays[0].error = Some("connection refused".into());
    net.daemon.info = None;

    let report = run_doctor(
        &dir.path().join("mortsom.toml"),
        &DoctorOptions::default(),
        &host,
        &net,
    )
    .await;

    assert_eq!(status_of(&report, "relay-port").status, CheckStatus::Warn);
    assert_eq!(
        status_of(&report, "relay ws://host.docker.internal:7000").status,
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
    let dir = example_config();
    let host = FakeHost::full();
    let mut net = FakeNet::healthy();
    let info = net.daemon.info.as_mut().unwrap();
    info.tags.insert("protocol_version".into(), "1".into());
    info.tags.remove("lnd_node_pubkey");

    let report = run_doctor(
        &dir.path().join("mortsom.toml"),
        &DoctorOptions::default(),
        &host,
        &net,
    )
    .await;

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
    let dir = example_config();
    let host = FakeHost::full();
    let mut net = FakeNet::healthy();
    net.daemon.relay_list = Some(RelayList {
        created_at: 1,
        relays: vec!["wss://relay.mostro.network".into()],
    });

    let report = run_doctor(
        &dir.path().join("mortsom.toml"),
        &DoctorOptions::default(),
        &host,
        &net,
    )
    .await;

    assert_eq!(
        status_of(&report, "mostro-relay-list").status,
        CheckStatus::Fail
    );
}

#[tokio::test]
async fn stale_info_event_warns() {
    let dir = example_config();
    let host = FakeHost::full();
    let mut net = FakeNet::healthy();
    net.daemon.info.as_mut().unwrap().created_at = 1;

    let report = run_doctor(
        &dir.path().join("mortsom.toml"),
        &DoctorOptions::default(),
        &host,
        &net,
    )
    .await;

    assert_eq!(
        status_of(&report, "mostro-daemon").status,
        CheckStatus::Warn
    );
    assert!(
        status_of(&report, "mostro-daemon")
            .detail
            .contains("older than")
    );
}

#[tokio::test]
async fn prebuilt_apk_skips_flutter_and_checks_the_file() {
    let dir = tempfile::tempdir().unwrap();
    let text = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../mortsom.toml.example"
    ))
    .unwrap()
    .replace("apk_path = \"\"", "apk_path = \"/tmp/app.apk\"");
    std::fs::write(dir.path().join("mortsom.toml"), text).unwrap();
    let mut host = FakeHost::full();
    host.tools.remove("flutter");

    let report = run_doctor(
        &dir.path().join("mortsom.toml"),
        &DoctorOptions { no_network: true },
        &host,
        &FakeNet::healthy(),
    )
    .await;

    assert_eq!(status_of(&report, "flutter").status, CheckStatus::Skip);
    assert_eq!(status_of(&report, "mobile-app").status, CheckStatus::Fail);
}

#[tokio::test]
async fn missing_config_file_fails_config_and_skips_network() {
    let host = FakeHost::full();

    let report = run_doctor(
        Path::new("/nonexistent/mortsom.toml"),
        &DoctorOptions::default(),
        &host,
        &FakeNet::healthy(),
    )
    .await;

    assert_eq!(status_of(&report, "config").status, CheckStatus::Fail);
    assert_eq!(status_of(&report, "network").status, CheckStatus::Skip);
    assert_eq!(status_of(&report, "system-image").status, CheckStatus::Skip);
}

#[tokio::test]
async fn docker_daemon_down_and_missing_driver_are_reported() {
    let dir = example_config();
    let mut host = FakeHost::full();
    host.outputs.insert(
        "docker info --format".into(),
        CommandOutput {
            success: false,
            stdout: String::new(),
            stderr: "Cannot connect to the Docker daemon\n".into(),
        },
    );
    host.outputs.insert("appium driver list".into(), ok("{}"));

    let report = run_doctor(
        &dir.path().join("mortsom.toml"),
        &DoctorOptions { no_network: true },
        &host,
        &FakeNet::healthy(),
    )
    .await;

    assert_eq!(status_of(&report, "docker").status, CheckStatus::Fail);
    assert!(
        status_of(&report, "docker")
            .detail
            .contains("Cannot connect")
    );
    assert_eq!(status_of(&report, "uiautomator2").status, CheckStatus::Fail);
}

#[tokio::test]
async fn wallet_liquidity_below_the_run_cap_fails_the_preflight() {
    let dir = example_config();
    let mut host = FakeHost::full();
    with_settings(&mut host, &dir);
    let path = dir.path().join("mortsom.toml");

    // Example caps: max_order_sats = 5000, max_run_spend_sats = 20000.
    let mut net = FakeNet::healthy();
    net.balance_sats = 7_000;
    let report = run_doctor(&path, &DoctorOptions::default(), &host, &net).await;
    assert_eq!(status_of(&report, "wallet alice").status, CheckStatus::Fail);
    assert!(
        status_of(&report, "wallet alice")
            .detail
            .contains("below the run cap (20000 sats)")
    );

    net.balance_sats = 100;
    let report = run_doctor(&path, &DoctorOptions::default(), &host, &net).await;
    assert_eq!(status_of(&report, "wallet bob").status, CheckStatus::Fail);
    assert!(!report.is_healthy());
}

/// The info event is republished on the daemon's own long interval (an hour
/// by default), so its absence during a short probe says nothing about the
/// daemon's health when the daemon is demonstrably publishing: a fresh
/// kind-10002 on the same relay proves it is alive and connected. Only
/// silence on every front is a failure.
#[test]
fn a_missing_info_event_warns_while_the_daemon_is_demonstrably_alive() {
    let mut probe = FakeNet::healthy().daemon;
    let expected = vec!["ws://host.docker.internal:7000".to_owned()];
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
        url: "ws://127.0.0.1:7000".to_owned(),
        port_in_use: false,
    };

    let mut report = DoctorReport::default();
    net::evaluate_daemon_with_run_relay(&probe, &[], Some(&run_relay), now(), &mut report);
    assert_eq!(
        status_of(&report, "relay ws://host.docker.internal:7000").status,
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
        status_of(&report, "relay ws://host.docker.internal:7000").status,
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
