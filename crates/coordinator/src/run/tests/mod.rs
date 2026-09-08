#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::BTreeMap;
use std::sync::Mutex;

use async_trait::async_trait;
use mortsom_android_driver::appium::{AppiumError, SessionSpec};
use mortsom_android_driver::process::tests_support::FakeRunner;
use mortsom_android_driver::ui::{UiServer, UiSession};
use mortsom_app_contract::{
    AdapterError, Apk, AppOrderState, AppProfile, BuildContext, CancelOutcome, Capability,
    CapabilitySet, DeviceContext, Identity, OrderDraft, OrderReference, OrderState, PaymentHandoff,
    PaymentPath, VisibleOrder, WalletConnection,
};
use mortsom_environment::docker::{ContainerRuntime, ContainerSpec, DockerError};
use mortsom_observers::wallet::PaymentState;
use mortsom_scenario_kit::registry::{ManualPayments, ScenarioFuture};
use nostr_sdk::prelude::{EventBuilder, FinalizeEvent, Keys, Kind, PublicKey, Tag};

use super::*;
use crate::safety::{MAINNET_CONFIRM_ENV, MAINNET_CONFIRM_VALUE};
use mortsom_common::Secret;
use mortsom_observers::relay::ObserverScope;

const HEX: &str = "8171eb680049f62ce8747d203f57c3f71de6eea308af6f995363896a1e47cc51";
const NSEC: &str = "nsec13as48eum93hkg7plv526r9gjpa0uc52zysqm93pmnkca9e69x6tsdjmdxd";
const ALICE: &str = "nostr+walletconnect://b889ff5b1513b641e2a139f661a661364979c5beee91842f8f0ef42ab558e9d4?relay=wss%3A%2F%2Frelay.damus.io&secret=71a8c14c1407c113601079c4302dab36460f0ccd0ad506f1f2dc73b5100e4f3c";
const BOB: &str = "nostr+walletconnect://b889ff5b1513b641e2a139f661a661364979c5beee91842f8f0ef42ab558e9d4?relay=wss%3A%2F%2Frelay.damus.io&secret=0000000000000000000000000000000000000000000000000000000000000001";
/// BOLT 11 reference invoice (payment hash 0001..0102).
const INVOICE: &str = "lnbc2500u1pvjluezsp5zyg3zyg3zyg3zyg3zyg3zyg3zyg3zyg3zyg3zyg3zyg3zyg3zygspp5qqqsyqcyq5rqwzqfqqqsyqcyq5rqwzqfqqqsyqcyq5rqwzqfqypqdq5xysxxatsyp3k7enxv4jsxqzpu9qrsgquk0rl77nj30yxdy8j9vdx85fkpmdla2087ne0xh8nhedh8w27kyke0lp53ut353s06fv3qfegext0eh0ymjpf39tuven09sam30g4vgpfna3rh";

struct World {
    dir: tempfile::TempDir,
    config: Config,
}

fn world(allow_mainnet: bool) -> World {
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
    .replace("health_timeout_secs = 120", "health_timeout_secs = 1")
    .replace("state_timeout_secs = 180", "state_timeout_secs = 5")
    .replace("settle_timeout_secs = 600", "settle_timeout_secs = 5")
    .replace(
        "expiration_timeout_secs = 1200",
        "expiration_timeout_secs = 5",
    )
    .replace("quiet_window_secs = 20", "quiet_window_secs = 1")
    .replace("background_secs = 15", "background_secs = 1")
    .replace("outage_secs = 20", "outage_secs = 1")
    .replace("host_port = 7000", "host_port = 47500")
    .replace(
        "allow_mainnet = false",
        &format!("allow_mainnet = {allow_mainnet}"),
    )
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

fn env_for(world: &World, confirm: bool) -> impl Fn(&str) -> Option<String> + '_ {
    move |name: &str| match name {
        "ANDROID_HOME" => Some(world.dir.path().join("sdk").display().to_string()),
        "ANDROID_AVD_HOME" => Some(world.dir.path().join("avd").display().to_string()),
        MAINNET_CONFIRM_ENV if confirm => Some(MAINNET_CONFIRM_VALUE.into()),
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
    restarted: Mutex<Vec<String>>,
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
    async fn restart(&self, name: &str) -> Result<(), DockerError> {
        self.restarted.lock().unwrap().push(name.to_owned());
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

/// UI stack that always answers; the fake adapter ignores it.
/// The fakes share one connectivity flag with the adapter: an app whose
/// device has no network cannot reach its wallet, and a double that ignores
/// that cannot exercise the scenarios which cut it. The second flag is the
/// driver itself going silent: every request then sits on its own HTTP
/// timeout, which is how a screenshot can cost a minute.
#[derive(Clone, Default)]
struct Flags {
    online: Arc<std::sync::atomic::AtomicBool>,
    silent_driver: Arc<std::sync::atomic::AtomicBool>,
}
struct FakeUi(Flags);
struct FakeServer(Flags);
struct FakeSession(Flags);
#[async_trait]
impl UiDriver for FakeUi {
    async fn start_server(
        &self,
        _: &dyn CommandRunner,
        _: u16,
        _: &Path,
        _: Duration,
    ) -> Result<Box<dyn UiServer>, AppiumError> {
        Ok(Box::new(FakeServer(self.0.clone())))
    }
}
#[async_trait]
impl UiServer for FakeServer {
    async fn create_session(&self, _: &SessionSpec) -> Result<Box<dyn UiSession>, AppiumError> {
        Ok(Box::new(FakeSession(self.0.clone())))
    }
    async fn stop(&self) -> Result<(), AppiumError> {
        Ok(())
    }
}
impl FakeSession {
    /// A WebDriver request against a driver that stopped answering returns
    /// only when its own HTTP timeout expires.
    async fn wait_out_the_http_timeout(&self) {
        if self
            .0
            .silent_driver
            .load(std::sync::atomic::Ordering::SeqCst)
        {
            tokio::time::sleep(Duration::from_secs(60)).await;
        }
    }
}

#[async_trait]
impl UiSession for FakeSession {
    async fn exists(&self, _: &str) -> Result<bool, AppiumError> {
        Ok(true)
    }
    async fn wait_for(&self, _: &str, _: Duration) -> Result<(), AppiumError> {
        Ok(())
    }
    async fn wait_gone(&self, _: &str, _: Duration) -> Result<(), AppiumError> {
        Ok(())
    }
    async fn tap(&self, _: &str) -> Result<(), AppiumError> {
        Ok(())
    }
    async fn type_text(&self, _: &str, _: &str) -> Result<(), AppiumError> {
        Ok(())
    }
    async fn text_of(&self, _: &str) -> Result<String, AppiumError> {
        Ok(String::new())
    }
    async fn is_enabled(&self, _: &str) -> Result<bool, AppiumError> {
        Ok(true)
    }
    async fn label_of(&self, _: &str) -> Result<String, AppiumError> {
        Ok(String::new())
    }
    async fn screenshot_png(&self) -> Result<Vec<u8>, AppiumError> {
        self.wait_out_the_http_timeout().await;
        Ok(b"\x89PNG".to_vec())
    }
    async fn page_source(&self) -> Result<String, AppiumError> {
        self.wait_out_the_http_timeout().await;
        Ok(format!("<hierarchy>{INVOICE}</hierarchy>"))
    }
    async fn background(&self, _: u64) -> Result<(), AppiumError> {
        Ok(())
    }
    async fn terminate_app(&self) -> Result<(), AppiumError> {
        Ok(())
    }
    async fn activate_app(&self) -> Result<(), AppiumError> {
        Ok(())
    }
    async fn clear_app(&self) -> Result<(), AppiumError> {
        Ok(())
    }
    async fn set_connectivity(&self, enabled: bool) -> Result<(), AppiumError> {
        self.0
            .online
            .store(enabled, std::sync::atomic::Ordering::SeqCst);
        Ok(())
    }
    async fn hide_keyboard(&self) -> Result<(), AppiumError> {
        Ok(())
    }
    async fn set_orientation(&self, _: bool) -> Result<(), AppiumError> {
        Ok(())
    }
    async fn end(&self) -> Result<(), AppiumError> {
        Ok(())
    }
}

/// A fake app: order state advances with each business action.
struct FakeAdapter {
    /// Shared with the fake UI: connectivity, and whether the driver has
    /// gone silent.
    flags: Flags,
    orders: Mutex<BTreeMap<String, OrderState>>,
    capabilities: CapabilitySet,
    fail_release: bool,
    cancelled: Mutex<Vec<String>>,
    /// Orders where one side already asked for a cooperative cancel.
    coop_pending: Mutex<Vec<String>>,
    /// Orders created so far (ids are `o1`, `o2`, ...).
    created: Mutex<u32>,
    /// Cancel is accepted by the UI but the daemon never closes the order.
    stubborn: bool,
    /// The driver stops answering at the moment the scenario fails.
    silence_driver_on_failure: bool,
}

impl FakeAdapter {
    fn full() -> Self {
        Self {
            flags: Flags {
                online: Arc::new(std::sync::atomic::AtomicBool::new(true)),
                silent_driver: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            },
            orders: Mutex::new(BTreeMap::new()),
            capabilities: CapabilitySet::new(Capability::ALL),
            fail_release: false,
            cancelled: Mutex::new(Vec::new()),
            coop_pending: Mutex::new(Vec::new()),
            created: Mutex::new(0),
            stubborn: false,
            silence_driver_on_failure: false,
        }
    }
    fn set(&self, id: &str, state: OrderState) {
        self.orders.lock().unwrap().insert(id.into(), state);
    }
}

#[async_trait]
impl AppAdapter for FakeAdapter {
    fn profile(&self) -> AppProfile {
        AppProfile {
            name: "fake".into(),
            package: "fake.app".into(),
        }
    }
    fn capabilities(&self) -> CapabilitySet {
        self.capabilities.clone()
    }
    async fn build(&self, _: &BuildContext, _: &dyn CommandRunner) -> Result<Apk, AdapterError> {
        Err(AdapterError::infrastructure("no build in tests"))
    }
    async fn onboard(
        &self,
        device: &DeviceContext<'_>,
        _: &TestEnvironment,
    ) -> Result<Identity, AdapterError> {
        Ok(Identity {
            public_key: format!("npub1{}", device.actor),
        })
    }
    async fn displayed_identity(
        &self,
        device: &DeviceContext<'_>,
    ) -> Result<Identity, AdapterError> {
        Ok(Identity {
            public_key: format!("npub1{}", device.actor),
        })
    }
    async fn configure_node_and_relay(
        &self,
        _: &DeviceContext<'_>,
        _: &TestEnvironment,
    ) -> Result<(), AdapterError> {
        Ok(())
    }
    async fn configure_nwc(
        &self,
        _: &DeviceContext<'_>,
        label: &str,
        _: &Secret,
    ) -> Result<WalletConnection, AdapterError> {
        Ok(WalletConnection {
            label: label.into(),
            connected: true,
        })
    }
    async fn create_order(
        &self,
        _: &DeviceContext<'_>,
        _: &OrderDraft,
    ) -> Result<VisibleOrder, AdapterError> {
        let mut created = self.created.lock().unwrap();
        *created += 1;
        let id = format!("o{created}");
        drop(created);
        self.set(&id, OrderState::Pending);
        Ok(VisibleOrder {
            id: Some(id),
            state: OrderState::Pending,
        })
    }
    async fn find_and_take_order(
        &self,
        _: &DeviceContext<'_>,
        order: &OrderReference,
    ) -> Result<(), AdapterError> {
        self.set(&order.id, OrderState::Taken);
        Ok(())
    }
    async fn pay_hold_invoice(
        &self,
        _: &DeviceContext<'_>,
        order: &OrderReference,
        _: PaymentPath,
    ) -> Result<PaymentHandoff, AdapterError> {
        if !self.flags.online.load(std::sync::atomic::Ordering::SeqCst) {
            return Err(AdapterError::wallet(
                "the wallet cannot be reached while the device is offline",
            ));
        }
        self.set(&order.id, OrderState::Active);
        Ok(PaymentHandoff {
            kind: PaymentPath::Nwc,
            invoice: Some(Secret::new(INVOICE)),
        })
    }
    async fn provide_buyer_invoice(
        &self,
        _: &DeviceContext<'_>,
        _: &OrderReference,
        _: Option<&Secret>,
    ) -> Result<Option<Secret>, AdapterError> {
        Ok(Some(Secret::new(INVOICE)))
    }
    async fn confirm_fiat_sent(
        &self,
        _: &DeviceContext<'_>,
        order: &OrderReference,
    ) -> Result<(), AdapterError> {
        self.set(&order.id, OrderState::FiatSent);
        Ok(())
    }
    async fn release(
        &self,
        _: &DeviceContext<'_>,
        order: &OrderReference,
    ) -> Result<(), AdapterError> {
        if self.fail_release {
            if self.silence_driver_on_failure {
                self.flags
                    .silent_driver
                    .store(true, std::sync::atomic::Ordering::SeqCst);
            }
            return Err(AdapterError::infrastructure("appium session died"));
        }
        self.set(&order.id, OrderState::Completed);
        Ok(())
    }
    async fn cancel_if_open(
        &self,
        device: &DeviceContext<'_>,
        order: &OrderReference,
    ) -> Result<CancelOutcome, AdapterError> {
        let mut orders = self.orders.lock().unwrap();
        let taker = device.actor == mortsom_android_driver::avd::Actor::Bob;
        match orders.get(&order.id).copied() {
            None => Ok(CancelOutcome::NotFound),
            Some(state) if state.is_terminal() => Ok(CancelOutcome::AlreadyClosed),
            // Mostro republishes an unfunded order the taker walks away from.
            Some(OrderState::Taken) if taker && !self.stubborn => {
                orders.insert(order.id.clone(), OrderState::Pending);
                Ok(CancelOutcome::Cancelled)
            }
            // A funded order needs both sides: the first request only marks it.
            Some(OrderState::Active | OrderState::FiatSent)
                if !self.coop_pending.lock().unwrap().contains(&order.id) =>
            {
                self.coop_pending.lock().unwrap().push(order.id.clone());
                Ok(CancelOutcome::Cancelled)
            }
            Some(_) if self.stubborn => Ok(CancelOutcome::Cancelled),
            Some(_) => {
                orders.insert(order.id.clone(), OrderState::Cancelled);
                self.cancelled.lock().unwrap().push(order.id.clone());
                Ok(CancelOutcome::Cancelled)
            }
        }
    }
    async fn visible_actions(
        &self,
        device: &DeviceContext<'_>,
        order: &OrderReference,
    ) -> Result<Vec<String>, AdapterError> {
        // Alice is always the seller/maker in these fakes.
        let state = self.orders.lock().unwrap().get(&order.id).copied();
        let seller = device.actor == mortsom_android_driver::avd::Actor::Alice;
        Ok(match (state, seller) {
            (Some(OrderState::Taken), true) => vec!["pay-invoice".into(), "cancel".into()],
            (Some(OrderState::Taken), false) => vec!["cancel".into()],
            (Some(OrderState::Active), true) => vec!["cancel".into(), "dispute".into()],
            (Some(OrderState::Active), false) => vec!["fiat-sent".into(), "cancel".into()],
            (Some(OrderState::FiatSent), true) => vec!["release".into(), "dispute".into()],
            _ => vec![],
        })
    }
    async fn cancel_payment(
        &self,
        _: &DeviceContext<'_>,
        order: &OrderReference,
    ) -> Result<(), AdapterError> {
        self.set(&order.id, OrderState::Cancelled);
        Ok(())
    }
    async fn observe_order(
        &self,
        _: &DeviceContext<'_>,
        order: &OrderReference,
    ) -> Result<AppOrderState, AdapterError> {
        let state = self
            .orders
            .lock()
            .unwrap()
            .get(&order.id)
            .copied()
            .ok_or_else(|| AdapterError::contract("unknown order"))?;
        Ok(AppOrderState {
            state,
            raw_label: state.as_str().into(),
        })
    }
}

/// Wallet reader that settles everything.
struct SettledReader;
#[async_trait]
impl WalletReader for SettledReader {
    async fn lookup(&self, _: &str) -> Result<Option<(u64, u64, PaymentState)>, String> {
        Ok(Some((1000, 1, PaymentState::Settled)))
    }
}

/// Detached observer plus a task that keeps publishing order states so the
/// relay correlation of any scenario is satisfied.
/// Order ids the fake daemon publishes states for.
const FAKE_ORDER_IDS: [&str; 12] = [
    "o1", "o2", "o3", "o4", "o5", "o6", "o7", "o8", "o9", "o10", "o11", "o12",
];

struct FakeObserver;
#[async_trait]
impl ObserverFactory for FakeObserver {
    async fn start(&self, _: &str, pubkey: PublicKey) -> Result<Arc<RelayObserver>, String> {
        let observer = Arc::new(RelayObserver::detached(ObserverScope::daemon_only(pubkey)));
        let keys = Keys::parse(NSEC).unwrap();
        let feeder = Arc::clone(&observer);
        tokio::spawn(async move {
            for _ in 0..200 {
                for (id, status) in FAKE_ORDER_IDS.iter().flat_map(|id| {
                    [
                        "pending",
                        "waiting-payment",
                        "active",
                        "fiat-sent",
                        "success",
                        "canceled",
                    ]
                    .into_iter()
                    .map(move |s| (*id, s))
                }) {
                    let event = EventBuilder::new(Kind::Custom(38383), "")
                        .tags([
                            Tag::parse(["d", id]).unwrap(),
                            Tag::parse(["s", status]).unwrap(),
                        ])
                        .finalize(&keys)
                        .unwrap();
                    feeder.ingest("ws://fake", &event);
                }
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        });
        Ok(observer)
    }
}

async fn run(
    world: &World,
    adapter: Arc<FakeAdapter>,
    options: RunOptions,
    confirm: bool,
) -> Result<RunSummary, String> {
    let ui = FakeUi(adapter.flags.clone());
    let docker = Arc::new(FakeDocker::default());
    let runner = runner();
    let env = env_for(world, confirm);
    fn wallets<'b>(
        _: &'b str,
        _: &'b mortsom_environment::WalletConfig,
    ) -> futures::future::BoxFuture<'b, Result<crate::run::WalletHandles, String>> {
        Box::pin(async {
            Ok(crate::run::WalletHandles {
                reader: Arc::new(SettledReader),
                node: None,
                network: None,
            })
        })
    }
    let deps = RunDeps {
        net: None,
        runner: &runner,
        ui: &ui,
        docker: Arc::clone(&docker) as Arc<dyn ContainerRuntime>,
        probe: Arc::new(AlwaysReady),
        adapter: adapter as Arc<dyn AppAdapter>,
        env: &env,
        wallets: &wallets,
        observer: &FakeObserver,
    };
    let result = run_scenarios(&world.config, &options, &deps).await;
    assert!(docker.running.lock().unwrap().is_empty(), "relay torn down");
    result
}

fn opts(names: &[&str]) -> RunOptions {
    RunOptions {
        desktop: None,
        stack: None,
        scenarios: names.iter().map(|s| (*s).to_owned()).collect(),
        tag: None,
        shutdown_grace: Some(Duration::from_millis(10)),
    }
}

mod behavior;
mod desktop;

mod phase5;
