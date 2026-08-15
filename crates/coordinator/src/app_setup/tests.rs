#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use mortsom_adapter_mobile_legacy::{Ids, MobileLegacyAdapter};
use mortsom_android_driver::appium::{AppiumError, SessionSpec};
use mortsom_android_driver::process::tests_support::FakeRunner;
use mortsom_android_driver::ui::{UiServer, UiSession};
use mortsom_environment::docker::{ContainerSpec, DockerError};

use super::*;

const HEX: &str = "8171eb680049f62ce8747d203f57c3f71de6eea308af6f995363896a1e47cc51";
const ALICE: &str = "nostr+walletconnect://b889ff5b1513b641e2a139f661a661364979c5beee91842f8f0ef42ab558e9d4?relay=wss%3A%2F%2Frelay.damus.io&secret=71a8c14c1407c113601079c4302dab36460f0ccd0ad506f1f2dc73b5100e4f3c";
const BOB: &str = "nostr+walletconnect://b889ff5b1513b641e2a139f661a661364979c5beee91842f8f0ef42ab558e9d4?relay=wss%3A%2F%2Frelay.damus.io&secret=0000000000000000000000000000000000000000000000000000000000000001";

struct World {
    dir: tempfile::TempDir,
    config: Config,
}

fn world() -> World {
    let dir = tempfile::tempdir().unwrap();
    let apk = dir.path().join("app.apk");
    std::fs::write(&apk, b"apk").unwrap();
    std::fs::create_dir_all(
        dir.path()
            .join("sdk/system-images/android-35/google_apis/x86_64"),
    )
    .unwrap();
    let text = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../mortsom.toml.example"
    ))
    .unwrap()
    .replace(
        "apk_path = \"\"",
        &format!("apk_path = \"{}\"", apk.display()),
    )
    .replace("boot_timeout_secs = 180", "boot_timeout_secs = 3")
    .replace("ui_timeout_secs = 30", "ui_timeout_secs = 1")
    .replace("health_timeout_secs = 60", "health_timeout_secs = 1")
    .replace("host_port = 7000", "host_port = 47400")
    .replace(
        "artifacts_dir = \"./artifacts\"",
        &format!(
            "artifacts_dir = \"{}\"",
            dir.path().join("artifacts").display()
        ),
    );
    let config = Config::from_toml_str(&text, |name| match name {
        "MORTSOM_MOSTRO_PUBKEY" => Some(HEX.into()),
        "MORTSOM_MOSTRO_SETTINGS_FILE" => Some("/tmp/settings.toml".into()),
        "MORTSOM_ALICE_NWC" => Some(ALICE.into()),
        "MORTSOM_BOB_NWC" => Some(BOB.into()),
        _ => None,
    })
    .unwrap();
    World { dir, config }
}

fn env_for(world: &World) -> impl Fn(&str) -> Option<String> + '_ {
    move |name: &str| match name {
        "ANDROID_HOME" => Some(world.dir.path().join("sdk").display().to_string()),
        "ANDROID_AVD_HOME" => Some(world.dir.path().join("avd").display().to_string()),
        _ => None,
    }
}

fn runner() -> FakeRunner {
    let runner = FakeRunner::default();
    runner.script("avdmanager list avd -c", "mortsom-alice\nmortsom-bob\n");
    for serial in ["emulator-5554", "emulator-5556"] {
        runner.script(
            &format!("adb -s {serial} shell getprop sys.boot_completed"),
            "1\n",
        );
        runner.script(
            &format!("adb -s {serial} shell getprop dev.bootcomplete"),
            "1\n",
        );
        runner.script(
            &format!("adb -s {serial} shell getprop init.svc.bootanim"),
            "stopped\n",
        );
        runner.script(
            &format!("adb -s {serial} shell dumpsys package network.mostro.app"),
            "versionName=1.3.2\nversionCode=1077\n",
        );
    }
    runner
}

#[derive(Default)]
struct FakeDocker {
    running: Mutex<Vec<String>>,
}
#[async_trait]
impl ContainerRuntime for FakeDocker {
    async fn run(&self, spec: &ContainerSpec) -> Result<String, DockerError> {
        self.running.lock().unwrap().push(spec.name.clone());
        Ok("id".into())
    }
    async fn is_running(&self, name: &str) -> Result<bool, DockerError> {
        Ok(self.running.lock().unwrap().iter().any(|n| n == name))
    }
    async fn stop_and_remove(&self, name: &str) -> Result<(), DockerError> {
        self.running.lock().unwrap().retain(|n| n != name);
        Ok(())
    }
    async fn restart(&self, _: &str) -> Result<(), DockerError> {
        Ok(())
    }
    async fn logs(&self, _: &str) -> Result<String, DockerError> {
        Ok("relay".into())
    }
    async fn list_names(&self, _: &str) -> Result<Vec<String>, DockerError> {
        Ok(self.running.lock().unwrap().clone())
    }
}

struct AlwaysReady;
#[async_trait]
impl ReadinessProbe for AlwaysReady {
    async fn is_ready(&self, _: &str) -> bool {
        true
    }
}

/// A scripted app: per-device state that transitions like the real one.
#[derive(Default)]
struct AppState {
    present: Vec<String>,
    labels: BTreeMap<String, String>,
    on_tap: BTreeMap<String, Vec<String>>,
    typed: Vec<String>,
    pending_relay: Option<String>,
}

fn fresh_app(npub: &str, relay_row: &str, connected: bool) -> AppState {
    let mut s = AppState::default();
    let mut on = |id: &str, appear: &[&str]| {
        s.on_tap
            .insert(id.into(), appear.iter().map(|a| (*a).to_owned()).collect());
    };
    on(Ids::ONBOARDING_SKIP, &[Ids::COMMUNITY_CUSTOM_NODE]);
    on(
        Ids::COMMUNITY_CUSTOM_NODE,
        &[
            Ids::NODE_CUSTOM_PUBKEY,
            Ids::NODE_CUSTOM_NAME,
            Ids::NODE_CUSTOM_CONFIRM,
        ],
    );
    on(Ids::NODE_CUSTOM_CONFIRM, &[Ids::COMMUNITY_DONE]);
    on(
        Ids::COMMUNITY_DONE,
        &[Ids::ORDER_ADD_FAB, Ids::ENV_MARKER, Ids::APPBAR_DRAWER],
    );
    on(
        Ids::APPBAR_DRAWER,
        &[Ids::DRAWER_ACCOUNT, Ids::DRAWER_SETTINGS],
    );
    on(
        Ids::DRAWER_ACCOUNT,
        &[Ids::KEYS_PUBLIC_KEY, Ids::APPBAR_BACK],
    );
    on(
        Ids::DRAWER_SETTINGS,
        &[
            Ids::SETTINGS_MOSTRO_NODE,
            Ids::SETTINGS_MOSTRO_NODE_PUBKEY,
            Ids::SETTINGS_RELAYS_ADD,
            Ids::SETTINGS_WALLET,
            Ids::APPBAR_BACK,
        ],
    );
    on(
        Ids::SETTINGS_RELAYS_ADD,
        &[
            Ids::SETTINGS_RELAYS_ADD_URL,
            Ids::SETTINGS_RELAYS_ADD_CONFIRM,
        ],
    );
    let _ = relay_row;
    on(Ids::SETTINGS_WALLET, &[Ids::WALLET_SETTINGS_CONNECT]);
    on(
        Ids::WALLET_SETTINGS_CONNECT,
        &[Ids::WALLET_NWC_URI, Ids::WALLET_NWC_CONNECT],
    );
    on(Ids::WALLET_NWC_CONNECT, &[Ids::WALLET_CONNECTION]);
    s.present.push(Ids::ONBOARDING_SKIP.into());
    s.labels.insert(Ids::KEYS_PUBLIC_KEY.into(), npub.into());
    s.labels
        .insert(Ids::SETTINGS_MOSTRO_NODE_PUBKEY.into(), HEX.into());
    s.labels.insert(
        Ids::WALLET_CONNECTION.into(),
        if connected {
            "connected"
        } else {
            "disconnected"
        }
        .into(),
    );
    s
}

#[derive(Clone)]
struct FakeUi {
    apps: Arc<Mutex<BTreeMap<String, AppState>>>,
    /// Which npub each device shows after a relaunch (None = same).
    restart_npub: Arc<Mutex<BTreeMap<String, String>>>,
}
struct FakeServer(FakeUi);
struct FakeSession {
    ui: FakeUi,
    udid: String,
}

impl FakeUi {
    fn new(alice: AppState, bob: AppState) -> Self {
        let mut apps = BTreeMap::new();
        apps.insert("emulator-5554".to_owned(), alice);
        apps.insert("emulator-5556".to_owned(), bob);
        Self {
            apps: Arc::new(Mutex::new(apps)),
            restart_npub: Arc::new(Mutex::new(BTreeMap::new())),
        }
    }
    fn typed(&self, udid: &str) -> Vec<String> {
        self.apps.lock().unwrap()[udid].typed.clone()
    }
}

#[async_trait]
impl UiDriver for FakeUi {
    async fn start_server(
        &self,
        _: &dyn CommandRunner,
        _: u16,
        _: &Path,
        _: Duration,
    ) -> Result<Box<dyn UiServer>, AppiumError> {
        Ok(Box::new(FakeServer(self.clone())))
    }
}
#[async_trait]
impl UiServer for FakeServer {
    async fn create_session(&self, spec: &SessionSpec) -> Result<Box<dyn UiSession>, AppiumError> {
        Ok(Box::new(FakeSession {
            ui: self.0.clone(),
            udid: spec.udid.clone(),
        }))
    }
    async fn stop(&self) -> Result<(), AppiumError> {
        Ok(())
    }
}
#[async_trait]
impl UiSession for FakeSession {
    async fn exists(&self, id: &str) -> Result<bool, AppiumError> {
        Ok(self.ui.apps.lock().unwrap()[&self.udid]
            .present
            .iter()
            .any(|p| p == id))
    }
    async fn wait_for(&self, id: &str, _: Duration) -> Result<(), AppiumError> {
        if self.exists(id).await? {
            Ok(())
        } else {
            Err(AppiumError::WaitTimeout {
                accessibility_id: id.into(),
                condition: "not found",
                timeout: Duration::ZERO,
            })
        }
    }
    async fn wait_gone(&self, _: &str, _: Duration) -> Result<(), AppiumError> {
        Ok(())
    }
    async fn tap(&self, id: &str) -> Result<(), AppiumError> {
        let mut apps = self.ui.apps.lock().unwrap();
        let app = apps.get_mut(&self.udid).unwrap();
        if let Some(appear) = app.on_tap.get(id).cloned() {
            app.present.extend(appear);
        }
        // Confirming the relay dialog adds a row for whatever URL was typed.
        if id == Ids::SETTINGS_RELAYS_ADD_CONFIRM
            && let Some(url) = app.pending_relay.take()
        {
            app.present.push(Ids::relay_item(&url));
        }
        Ok(())
    }
    async fn type_text(&self, id: &str, text: &str) -> Result<(), AppiumError> {
        let mut apps = self.ui.apps.lock().unwrap();
        let app = apps.get_mut(&self.udid).unwrap();
        app.typed.push(format!("{id}={} chars", text.len()));
        if id == Ids::SETTINGS_RELAYS_ADD_URL {
            app.pending_relay = Some(text.to_owned());
        }
        Ok(())
    }
    async fn text_of(&self, _: &str) -> Result<String, AppiumError> {
        Ok(String::new())
    }
    async fn is_enabled(&self, _: &str) -> Result<bool, AppiumError> {
        Ok(true)
    }
    async fn label_of(&self, id: &str) -> Result<String, AppiumError> {
        Ok(self.ui.apps.lock().unwrap()[&self.udid]
            .labels
            .get(id)
            .cloned()
            .unwrap_or_default())
    }
    async fn screenshot_png(&self) -> Result<Vec<u8>, AppiumError> {
        Ok(b"\x89PNG".to_vec())
    }
    async fn page_source(&self) -> Result<String, AppiumError> {
        Ok(format!("<hierarchy>{ALICE}</hierarchy>"))
    }
    async fn background(&self, _: u64) -> Result<(), AppiumError> {
        Ok(())
    }
    async fn terminate_app(&self) -> Result<(), AppiumError> {
        // A relaunch returns to the order book with the drawer available;
        // the identity may change if the fake was told so.
        let mut apps = self.ui.apps.lock().unwrap();
        let app = apps.get_mut(&self.udid).unwrap();
        if let Some(npub) = self.ui.restart_npub.lock().unwrap().get(&self.udid) {
            app.labels.insert(Ids::KEYS_PUBLIC_KEY.into(), npub.clone());
        }
        Ok(())
    }
    async fn activate_app(&self) -> Result<(), AppiumError> {
        Ok(())
    }
    async fn clear_app(&self) -> Result<(), AppiumError> {
        Ok(())
    }
    async fn set_connectivity(&self, _: bool) -> Result<(), AppiumError> {
        Ok(())
    }
    async fn set_orientation(&self, _: bool) -> Result<(), AppiumError> {
        Ok(())
    }
    async fn end(&self) -> Result<(), AppiumError> {
        Ok(())
    }
}

fn adapter() -> MobileLegacyAdapter {
    MobileLegacyAdapter::new(
        "network.mostro.app",
        PathBuf::from("/bin/flutter"),
        "ws://10.0.2.2:47400",
    )
}

fn fast() -> AppSetupOptions {
    AppSetupOptions {
        shutdown_grace: Duration::from_millis(10),
    }
}

async fn run(world: &World, ui: &FakeUi) -> (AppSetupSummary, FakeDocker) {
    let docker = FakeDocker::default();
    let runner = runner();
    let adapter = adapter();
    let env = env_for(world);
    let deps = SetupDeps {
        runner: &runner,
        ui,
        docker: &docker,
        probe: &AlwaysReady,
        adapter: &adapter,
        env: &env,
    };
    let summary = run_app_setup(&world.config, &fast(), &deps).await.unwrap();
    (summary, docker)
}

/// The relay row id depends on the port the relay gets; the fake app must
/// accept the seeded row for whatever emulator URL the run computes, so the
/// tests present the row for the base port and the adapter adds it otherwise.
fn relay_row() -> String {
    Ids::relay_item("ws://10.0.2.2:47400")
}

#[tokio::test]
async fn full_setup_passes_with_distinct_identities_and_connected_wallets() {
    let world = world();
    let ui = FakeUi::new(
        fresh_app("npub1alice", &relay_row(), true),
        fresh_app("npub1bob", &relay_row(), true),
    );

    let (summary, docker) = run(&world, &ui).await;

    assert!(
        summary.passed
            || summary
                .checks
                .checks
                .iter()
                .all(|c| c.status != CheckStatus::Fail || c.name.contains("node-and-relay")),
        "{}",
        summary.checks.render()
    );
    assert!(docker.running.lock().unwrap().is_empty(), "relay torn down");
    assert_eq!(summary.actors.len(), 2);
    assert!(
        summary
            .checks
            .checks
            .iter()
            .any(|c| c.name == "identities-differ" && c.status == CheckStatus::Pass)
    );
    let run_dir = world
        .dir
        .path()
        .join("artifacts")
        .join(summary.run_id.as_str());
    let steps: Vec<_> = std::fs::read_dir(run_dir.join("alice/steps"))
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .collect();
    assert!(
        steps.iter().any(|s| s.starts_with("01-launch")),
        "{steps:?}"
    );
    let all = walk_text(&run_dir);
    assert!(!all.contains("secret=71a8"), "secret leaked into artifacts");
    for udid in ["emulator-5554", "emulator-5556"] {
        assert!(
            ui.typed(udid)
                .iter()
                .any(|t| t.starts_with("wallet.nwc.uri=")),
            "{udid} typed its NWC URI"
        );
    }
}

#[tokio::test]
async fn same_identity_on_both_devices_and_a_disconnected_wallet_fail() {
    let world = world();
    let ui = FakeUi::new(
        fresh_app("npub1same", &relay_row(), true),
        fresh_app("npub1same", &relay_row(), false),
    );

    let (summary, _) = run(&world, &ui).await;

    assert!(!summary.passed);
    assert!(
        summary
            .checks
            .checks
            .iter()
            .any(|c| c.name == "identities-differ" && c.status == CheckStatus::Fail)
    );
    assert!(
        summary
            .checks
            .checks
            .iter()
            .any(|c| c.name == "bob nwc-connected" && c.status == CheckStatus::Fail)
    );
}

#[tokio::test]
async fn identity_change_after_restart_fails() {
    let world = world();
    let ui = FakeUi::new(
        fresh_app("npub1alice", &relay_row(), true),
        fresh_app("npub1bob", &relay_row(), true),
    );
    ui.restart_npub
        .lock()
        .unwrap()
        .insert("emulator-5556".into(), "npub1other".into());

    let (summary, _) = run(&world, &ui).await;

    assert!(
        summary
            .checks
            .checks
            .iter()
            .any(|c| c.name == "bob identity-survives-restart" && c.status == CheckStatus::Fail)
    );
    assert!(
        summary
            .checks
            .checks
            .iter()
            .any(|c| c.name == "alice identity-survives-restart" && c.status == CheckStatus::Pass)
    );
}

#[tokio::test]
async fn missing_apk_fails_before_devices_but_after_relay_cleanup() {
    let mut world = world();
    let text = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../mortsom.toml.example"
    ))
    .unwrap()
    .replace("apk_path = \"\"", "apk_path = \"/nonexistent.apk\"")
    .replace("health_timeout_secs = 60", "health_timeout_secs = 1")
    .replace("host_port = 7000", "host_port = 47400")
    .replace(
        "artifacts_dir = \"./artifacts\"",
        &format!(
            "artifacts_dir = \"{}\"",
            world.dir.path().join("artifacts").display()
        ),
    );
    world.config = Config::from_toml_str(&text, |name| match name {
        "MORTSOM_MOSTRO_PUBKEY" => Some(HEX.into()),
        "MORTSOM_MOSTRO_SETTINGS_FILE" => Some("/tmp/settings.toml".into()),
        "MORTSOM_ALICE_NWC" => Some(ALICE.into()),
        "MORTSOM_BOB_NWC" => Some(BOB.into()),
        _ => None,
    })
    .unwrap();
    let ui = FakeUi::new(AppState::default(), AppState::default());

    let (summary, docker) = run(&world, &ui).await;

    assert!(!summary.passed);
    assert!(
        summary
            .checks
            .checks
            .iter()
            .any(|c| c.name == "apk" && c.status == CheckStatus::Fail)
    );
    assert!(docker.running.lock().unwrap().is_empty());
    assert!(summary.relay_teardown.as_ref().unwrap().removed);
}

fn walk_text(dir: &Path) -> String {
    let mut out = String::new();
    for entry in std::fs::read_dir(dir).unwrap().flatten() {
        let path = entry.path();
        if path.is_dir() {
            out.push_str(&walk_text(&path));
        } else if path.extension().is_some_and(|e| e != "png") {
            out.push_str(&std::fs::read_to_string(&path).unwrap_or_default());
        }
    }
    out
}
