#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::BTreeMap;
use std::sync::Mutex;

use async_trait::async_trait;
use mortsom_app_contract::{
    AdapterError, AppOrderState, AppProfile, CancelOutcome, Capability, CapabilitySet,
    DeviceContext, Identity, OrderDraft, OrderReference, OrderState, PaymentHandoff, PaymentPath,
    VisibleOrder, WalletConnection,
};
use mortsom_environment::WalletConfig;
use mortsom_environment::daemon::{DaemonInfo, DaemonProbe, RelayList, RelayProbe};
use mortsom_environment::docker::{ContainerRuntime, ContainerSpec, DockerError};
use mortsom_environment::wallet::{WalletProbe, WalletProbeError};
use mortsom_observers::wallet::PaymentState;
use mortsom_scenario_kit::registry::{ManualPayments, ScenarioFuture};
use mortsom_ui_driver::error::UiError;
use mortsom_ui_driver::ui::UiSession;
use nostr_sdk::prelude::{EventBuilder, FinalizeEvent, Keys, Kind, PublicKey, Tag};

use super::*;
use crate::doctor::{NetProbes, PortState};
use crate::safety::{MAINNET_CONFIRM_ENV, MAINNET_CONFIRM_VALUE};
use mortsom_common::Actor;
use mortsom_common::Secret;
use mortsom_observers::relay::ObserverScope;

const HEX: &str = "8171eb680049f62ce8747d203f57c3f71de6eea308af6f995363896a1e47cc51";
const NSEC: &str = "nsec13as48eum93hkg7plv526r9gjpa0uc52zysqm93pmnkca9e69x6tsdjmdxd";
/// BOLT 11 reference invoice (payment hash 0001..0102).
const INVOICE: &str = "lnbc2500u1pvjluezsp5zyg3zyg3zyg3zyg3zyg3zyg3zyg3zyg3zyg3zyg3zyg3zyg3zygspp5qqqsyqcyq5rqwzqfqqqsyqcyq5rqwzqfqqqsyqcyq5rqwzqfqypqdq5xysxxatsyp3k7enxv4jsxqzpu9qrsgquk0rl77nj30yxdy8j9vdx85fkpmdla2087ne0xh8nhedh8w27kyke0lp53ut353s06fv3qfegext0eh0ymjpf39tuven09sam30g4vgpfna3rh";
const RELAY_URL: &str = "ws://127.0.0.1:7000";
const ALICE_LND: &str = "127.0.0.1:10001";
const BOB_LND: &str = "127.0.0.1:10002";

struct World {
    dir: tempfile::TempDir,
    config: Config,
}

/// A run against the stack: LND wallets Mortsom holds, the stack relay,
/// clients built from a captured Linux bundle. `allow_mainnet` opts the
/// configuration into the mainnet double opt-in; a run of nodes that report
/// regtest never needs it.
fn world(allow_mainnet: bool) -> World {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("home")).unwrap();
    for actor in ["alice", "bob"] {
        let creds = dir.path().join("creds").join(actor);
        std::fs::create_dir_all(&creds).unwrap();
        std::fs::write(creds.join("tls.cert"), b"cert").unwrap();
        std::fs::write(creds.join("admin.macaroon"), b"macaroon").unwrap();
    }
    let text = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../config/desktop.example.toml"
    ))
    .unwrap()
    .replace("timeout_secs = 30", "timeout_secs = 1")
    .replace("state_timeout_secs = 90", "state_timeout_secs = 5")
    .replace("settle_timeout_secs = 120", "settle_timeout_secs = 5")
    .replace(
        "expiration_timeout_secs = 900",
        "expiration_timeout_secs = 5",
    )
    .replace("quiet_window_secs = 10", "quiet_window_secs = 1")
    .replace("background_secs = 5", "background_secs = 1")
    .replace("outage_secs = 10", "outage_secs = 1")
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
    let creds = dir.path().join("creds");
    let config = Config::from_toml_str(&text, |name| match name {
        "MORTSOM_MOSTRO_PUBKEY" => Some(HEX.into()),
        "MORTSOM_MOSTRO_SETTINGS_FILE" => Some("/tmp/settings.toml".into()),
        "MORTSOM_ALICE_LND_HOST" => Some(ALICE_LND.into()),
        "MORTSOM_BOB_LND_HOST" => Some(BOB_LND.into()),
        "MORTSOM_ALICE_LND_CERT" => Some(creds.join("alice/tls.cert").display().to_string()),
        "MORTSOM_BOB_LND_CERT" => Some(creds.join("bob/tls.cert").display().to_string()),
        "MORTSOM_ALICE_LND_MACAROON" => {
            Some(creds.join("alice/admin.macaroon").display().to_string())
        }
        "MORTSOM_BOB_LND_MACAROON" => Some(creds.join("bob/admin.macaroon").display().to_string()),
        _ => None,
    })
    .unwrap();
    World { dir, config }
}

/// The public facts of the stack the fixture configuration is bound to.
fn stack() -> crate::stack::StackSnapshot {
    serde_json::from_value(serde_json::json!({
        "daemon_pubkey": HEX, "relay_url": RELAY_URL,
        "wallet_endpoints": {"alice": ALICE_LND, "bob": BOB_LND},
        "daemon_image": {"image_id": format!("sha256:{}", "a".repeat(64)),
            "reference": format!("mostrop2p/mostro@sha256:{}", "b".repeat(64)),
                "repository_digest": format!("sha256:{}", "b".repeat(64))},
        "daemon_source":{"kind":"pinned-image"},
        "declared_limits": {"order_expiration_secs": 300, "hold_invoice_expiry_secs": 120,
            "buyer_invoice_deadline_secs": 120, "payout_invoice_min_expiry_secs": 300}
    }))
    .unwrap()
}

/// A Linux bundle with every file the artifact check requires, captured
/// the way `desktop::prepare` captures a real build.
fn desktop_options(world: &World) -> DesktopOptions {
    let source = world.dir.path().join("bundle");
    for file in [
        "lib/libapp.so",
        "lib/libflutter_linux_gtk.so",
        "lib/librust.so",
        "lib/libflutter_secure_storage_linux_plugin.so",
        "lib/liburl_launcher_linux_plugin.so",
        "data/icudtl.dat",
        "data/flutter_assets/AssetManifest.bin",
    ] {
        let path = source.join(file);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, b"fixture").unwrap();
    }
    let executable = source.join("mostro");
    std::fs::write(&executable, b"#!/bin/sh\nexit 0\n").unwrap();
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    let bundle = crate::fingerprint::CapturedBundle::capture(&source).unwrap();
    DesktopOptions {
        recovery: None,
        resume: false,
        provenance: crate::provenance::BuildProvenance::prebuilt(),
        platform: mortsom_desktop_driver::Platform::Linux,
        artifact: bundle.root().join("mostro"),
        bundle: Arc::new(bundle),
        xvfb: "unused-Xvfb".into(),
        browser: "unused-browser".into(),
        relay_audit: None,
        chromedriver: "unused-driver".into(),
    }
}

fn env_for(world: &World, confirm: bool) -> impl Fn(&str) -> Option<String> + '_ {
    move |name: &str| match name {
        "HOME" => Some(world.dir.path().join("home").display().to_string()),
        MAINNET_CONFIRM_ENV if confirm => Some(MAINNET_CONFIRM_VALUE.into()),
        _ => None,
    }
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

/// A stack whose daemon, relay and nodes answer every preflight question.
struct ReadyNet;
#[async_trait]
impl NetProbes for ReadyNet {
    async fn local_port(&self, _: u16) -> PortState {
        PortState::InUse
    }
    async fn daemon(&self, _: &[String], _: PublicKey) -> DaemonProbe {
        DaemonProbe {
            relays: vec![RelayProbe {
                configured: RELAY_URL.into(),
                dialled: RELAY_URL.into(),
                error: None,
            }],
            info: Some(DaemonInfo {
                pubkey: HEX.into(),
                created_at: std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_secs(),
                tags: [
                    ("mostro_version", "0.18.7"),
                    ("protocol_version", "2"),
                    ("lnd_node_pubkey", "02abc"),
                    ("lnd_version", "0.19"),
                    ("lnd_node_alias", "stack"),
                ]
                .into_iter()
                .map(|(k, v)| (k.to_owned(), v.to_owned()))
                .collect(),
            }),
            relay_list: Some(RelayList {
                created_at: 1,
                relays: vec![RELAY_URL.into()],
            }),
        }
    }
    async fn wallet(&self, label: &str, _: &WalletConfig) -> Result<WalletProbe, WalletProbeError> {
        Ok(WalletProbe {
            label: label.into(),
            alias: Some(label.into()),
            network: Some("regtest".into()),
            methods: ["pay_invoice", "make_invoice", "lookup_invoice"]
                .map(str::to_owned)
                .to_vec(),
            balance_sats: 100_000,
            inbound_sats: Some(100_000),
        })
    }
}

/// UI stack that always answers; the fake adapter ignores it.
/// The fakes share one connectivity flag with the adapter: an app whose
/// client has no network cannot reach its wallet, and a double that ignores
/// that cannot exercise the scenarios which cut it. The second flag is the
/// driver itself going silent: every request then sits on its own HTTP
/// timeout, which is how a screenshot can cost a minute.
#[derive(Clone, Default)]
struct Flags {
    online: Arc<std::sync::atomic::AtomicBool>,
    silent_driver: Arc<std::sync::atomic::AtomicBool>,
}
/// Hands out fake sessions in place of Linux and Web clients.
struct FakeClients(Flags);
struct FakeSession(Flags);
struct FakeWebServer;
#[async_trait]
impl DesktopServer for FakeWebServer {
    fn url(&self) -> &str {
        "http://127.0.0.1:12345"
    }
    async fn stop(&mut self) -> Result<(), String> {
        Ok(())
    }
}
#[async_trait]
impl DesktopFactory for FakeClients {
    async fn server(&self, _: &DesktopOptions) -> Result<Box<dyn DesktopServer>, String> {
        Ok(Box::new(FakeWebServer))
    }
    fn session(
        &self,
        _: Actor,
        _: &DesktopOptions,
        _: Option<&str>,
    ) -> Result<Arc<dyn UiSession>, String> {
        Ok(Arc::new(FakeSession(self.0.clone())))
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
    async fn exists(&self, _: &str) -> Result<bool, UiError> {
        Ok(true)
    }
    async fn wait_for(&self, _: &str, _: Duration) -> Result<(), UiError> {
        Ok(())
    }
    async fn wait_gone(&self, _: &str, _: Duration) -> Result<(), UiError> {
        Ok(())
    }
    async fn tap(&self, _: &str) -> Result<(), UiError> {
        Ok(())
    }
    async fn type_text(&self, _: &str, _: &str) -> Result<(), UiError> {
        Ok(())
    }
    async fn text_of(&self, _: &str) -> Result<String, UiError> {
        Ok(String::new())
    }
    async fn is_enabled(&self, _: &str) -> Result<bool, UiError> {
        Ok(true)
    }
    async fn label_of(&self, _: &str) -> Result<String, UiError> {
        Ok(String::new())
    }
    async fn screenshot_png(&self) -> Result<Vec<u8>, UiError> {
        self.wait_out_the_http_timeout().await;
        Ok(b"\x89PNG".to_vec())
    }
    async fn page_source(&self) -> Result<String, UiError> {
        self.wait_out_the_http_timeout().await;
        Ok(format!("<hierarchy>{INVOICE}</hierarchy>"))
    }
    async fn background(&self, _: u64) -> Result<(), UiError> {
        Ok(())
    }
    async fn terminate_app(&self) -> Result<(), UiError> {
        Ok(())
    }
    async fn activate_app(&self) -> Result<(), UiError> {
        Ok(())
    }
    async fn clear_app(&self) -> Result<(), UiError> {
        Ok(())
    }
    async fn set_connectivity(&self, enabled: bool) -> Result<(), UiError> {
        self.0
            .online
            .store(enabled, std::sync::atomic::Ordering::SeqCst);
        Ok(())
    }
    async fn hide_keyboard(&self) -> Result<(), UiError> {
        Ok(())
    }
    async fn set_orientation(&self, _: bool) -> Result<(), UiError> {
        Ok(())
    }
    async fn end(&self) -> Result<(), UiError> {
        Ok(())
    }
}

/// A fake app: order state advances with each business action.
struct FakeAdapter {
    recovery_calls: Mutex<Vec<String>>,
    observed_actors: Mutex<Vec<Actor>>,
    complete_after_observations: Option<usize>,
    /// Shared with the fake UI: connectivity, and whether the driver has
    /// gone silent.
    flags: Flags,
    /// Order states and payment hashes, shared with the fake nodes.
    ledger: Arc<Ledger>,
    capabilities: CapabilitySet,
    fail_release: bool,
    cancelled: Mutex<Vec<String>>,
    /// Orders where one side already asked for a cooperative cancel.
    coop_pending: Mutex<Vec<String>>,
    /// Orders created so far (ids are `o1`, `o2`, ...).
    created: Mutex<u32>,
    /// Cancel is accepted by the UI but the daemon never closes the order.
    stubborn: bool,
    /// The UI flips a funded order to `cancelled` on the maker's request
    /// alone, while the daemon still holds the escrow.
    ui_claims_cancelled: bool,
    /// App v2 behaviour observed live: the maker's own screen marks a funded
    /// order `cancelled` on her request alone, while the daemon holds the
    /// escrow and waits for the counterpart. Only Alice's view is affected.
    maker_screen_lies: bool,
    maker_lied_about: Mutex<Vec<String>>,
    /// The trades list never shows the order (app v2 on Web before its
    /// trades store existed): cancel finds nothing to act on.
    hides_trades: bool,
    /// The driver stops answering at the moment the scenario fails.
    silence_driver_on_failure: bool,
}

impl FakeAdapter {
    fn full() -> Self {
        Self {
            recovery_calls: Mutex::new(Vec::new()),
            observed_actors: Mutex::new(Vec::new()),
            complete_after_observations: None,
            flags: Flags {
                online: Arc::new(std::sync::atomic::AtomicBool::new(true)),
                silent_driver: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            },
            ledger: Arc::new(Ledger::default()),
            capabilities: CapabilitySet::new(Capability::ALL),
            fail_release: false,
            cancelled: Mutex::new(Vec::new()),
            coop_pending: Mutex::new(Vec::new()),
            created: Mutex::new(0),
            stubborn: false,
            ui_claims_cancelled: false,
            maker_screen_lies: false,
            maker_lied_about: Mutex::new(Vec::new()),
            hides_trades: false,
            silence_driver_on_failure: false,
        }
    }
    fn set(&self, id: &str, state: OrderState) {
        self.ledger.orders.lock().unwrap().insert(id.into(), state);
    }
    fn orders(&self) -> std::sync::MutexGuard<'_, BTreeMap<String, OrderState>> {
        self.ledger.orders.lock().unwrap()
    }
}

#[async_trait]
impl AppAdapter for FakeAdapter {
    fn profile(&self) -> AppProfile {
        AppProfile {
            name: "fake".into(),
        }
    }
    fn capabilities(&self) -> CapabilitySet {
        self.capabilities.clone()
    }
    async fn onboard(
        &self,
        device: &DeviceContext<'_>,
        _: &TestEnvironment,
    ) -> Result<Identity, AdapterError> {
        self.recovery_calls
            .lock()
            .unwrap()
            .push(format!("onboard:{}", device.actor));
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
        device: &DeviceContext<'_>,
        _: &TestEnvironment,
    ) -> Result<(), AdapterError> {
        self.recovery_calls
            .lock()
            .unwrap()
            .push(format!("configure:{}", device.actor));
        Ok(())
    }
    async fn existing_identity(
        &self,
        device: &DeviceContext<'_>,
    ) -> Result<Identity, AdapterError> {
        self.recovery_calls
            .lock()
            .unwrap()
            .push(format!("existing:{}", device.actor));
        let public_key = device
            .ui
            .text_of("identity")
            .await
            .map_err(|e| AdapterError::infrastructure(e.to_string()))?;
        Ok(Identity { public_key })
    }
    async fn owned_orders(&self, device: &DeviceContext<'_>) -> Result<Vec<String>, AdapterError> {
        self.recovery_calls
            .lock()
            .unwrap()
            .push(format!("owned:{}", device.actor));
        Ok(self.orders().keys().cloned().collect())
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
        let (invoice, hash) = invoice_for(1000);
        self.ledger
            .hashes
            .lock()
            .unwrap()
            .insert(hash, order.id.clone());
        Ok(PaymentHandoff {
            kind: PaymentPath::ExternalWallet,
            invoice: Some(invoice),
        })
    }
    async fn buyer_invoice_amount(
        &self,
        _: &DeviceContext<'_>,
        _: &OrderReference,
    ) -> Result<u64, AdapterError> {
        Ok(1000)
    }
    async fn provide_buyer_invoice(
        &self,
        _: &DeviceContext<'_>,
        _: &OrderReference,
        invoice: Option<&Secret>,
    ) -> Result<Option<Secret>, AdapterError> {
        Ok(Some(
            invoice.cloned().unwrap_or_else(|| invoice_for(1000).0),
        ))
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
            return Err(AdapterError::infrastructure("client session died"));
        }
        self.set(&order.id, OrderState::Completed);
        Ok(())
    }
    async fn cancel_if_open(
        &self,
        device: &DeviceContext<'_>,
        order: &OrderReference,
    ) -> Result<CancelOutcome, AdapterError> {
        let mut orders = self.orders();
        let taker = device.actor == Actor::Bob;
        if self.hides_trades {
            return Ok(CancelOutcome::NotFound);
        }
        if !taker && self.maker_lied_about.lock().unwrap().contains(&order.id) {
            return Ok(CancelOutcome::AlreadyClosed);
        }
        match orders.get(&order.id).copied() {
            None => Ok(CancelOutcome::NotFound),
            Some(state) if state.is_terminal() => Ok(CancelOutcome::AlreadyClosed),
            Some(OrderState::PayoutPending) => Err(AdapterError::contract(
                "payout-pending has no cancel control",
            )),
            // Mostro republishes an unfunded order the taker walks away from.
            Some(OrderState::Taken) if taker && !self.stubborn => {
                orders.insert(order.id.clone(), OrderState::Pending);
                Ok(CancelOutcome::Cancelled)
            }
            Some(OrderState::Active | OrderState::FiatSent) if !taker && self.maker_screen_lies => {
                self.coop_pending.lock().unwrap().push(order.id.clone());
                self.maker_lied_about.lock().unwrap().push(order.id.clone());
                Ok(CancelOutcome::AlreadyClosed)
            }
            Some(OrderState::Active | OrderState::FiatSent) if self.ui_claims_cancelled => {
                orders.insert(order.id.clone(), OrderState::Cancelled);
                self.cancelled.lock().unwrap().push(order.id.clone());
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
        let state = self.orders().get(&order.id).copied();
        let seller = device.actor == Actor::Alice;
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
        device: &DeviceContext<'_>,
        order: &OrderReference,
    ) -> Result<AppOrderState, AdapterError> {
        self.observed_actors.lock().unwrap().push(device.actor);
        if self
            .complete_after_observations
            .is_some_and(|count| self.observed_actors.lock().unwrap().len() >= count)
        {
            self.set(&order.id, OrderState::Completed);
        }
        let mut state = self
            .orders()
            .get(&order.id)
            .copied()
            .ok_or_else(|| AdapterError::contract("unknown order"))?;
        if device.actor == Actor::Alice
            && !state.is_terminal()
            && self.maker_lied_about.lock().unwrap().contains(&order.id)
        {
            state = OrderState::Cancelled;
        }
        Ok(AppOrderState {
            state,
            raw_label: state.as_str().into(),
        })
    }
}

/// A fresh regtest invoice for `sats`, signed by a fixed throwaway key.
/// Every call carries a new payment hash, as every real invoice does.
fn invoice_for(sats: u64) -> (Secret, String) {
    use bitcoin::hashes::{Hash, sha256};
    use bitcoin::secp256k1::{Secp256k1, SecretKey};
    use lightning_invoice::{Currency, InvoiceBuilder, PaymentSecret};
    static NONCE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    let nonce = NONCE.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let secp = Secp256k1::new();
    let key = SecretKey::from_slice(&[0x11; 32]).unwrap();
    let preimage = [sats.to_be_bytes(), nonce.to_be_bytes()].concat();
    let invoice = InvoiceBuilder::new(Currency::Regtest)
        .description("mortsom fixture".into())
        .payment_hash(sha256::Hash::hash(&preimage))
        .payment_secret(PaymentSecret([0x22; 32]))
        .amount_milli_satoshis(sats * 1000)
        .current_timestamp()
        .min_final_cltv_expiry_delta(18)
        .build_signed(|hash| secp.sign_ecdsa_recoverable(hash, &key))
        .unwrap();
    let hash = invoice.payment_hash().to_string();
    (Secret::new(invoice.to_string()), hash)
}

/// What the fake daemon and wallets agree on: the state of every order and
/// which payment hash belongs to which order, so a wallet can answer for a
/// payment the way a real node would once the daemon has settled or
/// refunded it.
#[derive(Default)]
struct Ledger {
    orders: Mutex<BTreeMap<String, OrderState>>,
    hashes: Mutex<BTreeMap<String, String>>,
}

impl Ledger {
    fn state_of_hash(&self, hash: &str) -> Option<OrderState> {
        let order = self.hashes.lock().unwrap().get(hash).cloned()?;
        self.orders.lock().unwrap().get(&order).copied()
    }
    fn refunded(&self, hash: &str) -> bool {
        matches!(
            self.state_of_hash(hash),
            Some(OrderState::Cancelled | OrderState::Expired)
        )
    }
}

/// A node Mortsom holds: every payment settles at once, and one the daemon
/// refunded (a cancelled or expired order) reads as failed with no
/// settled record, as it would on a real node.
struct SettledNode(Arc<Ledger>);
#[async_trait]
impl WalletReader for SettledNode {
    async fn lookup(&self, hash: &str) -> Result<Option<(u64, u64, PaymentState)>, String> {
        let state = if self.0.refunded(hash) {
            PaymentState::Failed
        } else {
            PaymentState::Settled
        };
        Ok(Some((1000, 0, state)))
    }
}
#[async_trait]
impl NodeWallet for SettledNode {
    async fn cancel_invoice(&self, _: &str) -> Result<(), String> {
        Ok(())
    }
    async fn settled_record_count(&self, hash: &str, _: bool) -> Result<usize, String> {
        Ok(usize::from(!self.0.refunded(hash)))
    }
    async fn make_invoice(&self, sats: u64, memo: &str) -> Result<Secret, String> {
        let (invoice, hash) = invoice_for(sats);
        if let Some(order) = memo.strip_prefix("mortsom ") {
            self.0.hashes.lock().unwrap().insert(hash, order.to_owned());
        }
        Ok(invoice)
    }
    async fn start_payment(&self, _: &Secret) -> Result<(), String> {
        Ok(())
    }
}

/// Wallet handles for nodes on `network` sharing `ledger` with the app.
fn nodes_on(
    ledger: Arc<Ledger>,
    network: &'static str,
) -> impl for<'b> Fn(
    &'b str,
    &'b WalletConfig,
) -> futures::future::BoxFuture<'b, Result<WalletHandles, String>> {
    move |_, _| {
        let ledger = Arc::clone(&ledger);
        Box::pin(async move {
            Ok(WalletHandles {
                reader: Arc::new(SettledNode(Arc::clone(&ledger))),
                node: Some(Arc::new(SettledNode(ledger))),
                network: Some(network.to_owned()),
            })
        })
    }
}

/// Detached observer plus a task that keeps publishing order states so the
/// relay correlation of any scenario is satisfied.
/// Order ids the fake daemon publishes states for.
const FAKE_ORDER_IDS: [&str; 12] = [
    "o1", "o2", "o3", "o4", "o5", "o6", "o7", "o8", "o9", "o10", "o11", "o12",
];

/// Fake daemon on the relay. `publish_closed: false` models a daemon that
/// never closes an order (no `canceled`, no `success`) whatever the UI says.
struct FakeObserver {
    publish_closed: bool,
    /// When set, `canceled` is published for an order only after the
    /// adapter recorded a daemon-accepted cancellation of it.
    canceled_gate: Option<Arc<Mutex<Vec<String>>>>,
}
impl Default for FakeObserver {
    fn default() -> Self {
        Self {
            publish_closed: true,
            canceled_gate: None,
        }
    }
}
#[async_trait]
impl ObserverFactory for FakeObserver {
    async fn start(&self, _: &str, pubkey: PublicKey) -> Result<Arc<RelayObserver>, String> {
        let observer = Arc::new(RelayObserver::detached(ObserverScope::daemon_only(pubkey)));
        let keys = Keys::parse(NSEC).unwrap();
        let feeder = Arc::clone(&observer);
        let publish_closed = self.publish_closed;
        let gate = self.canceled_gate.clone();
        tokio::spawn(async move {
            for _ in 0..200 {
                for (id, status) in FAKE_ORDER_IDS.iter().flat_map(|id| {
                    // What the live daemon puts on the public book: taken,
                    // funded and fiat-sent orders all read `in-progress`.
                    ["pending", "in-progress", "success", "canceled"]
                        .into_iter()
                        .filter(move |s| publish_closed || !matches!(*s, "canceled" | "success"))
                        .map(move |s| (*id, s))
                }) {
                    if status == "canceled"
                        && gate
                            .as_ref()
                            .is_some_and(|g| !g.lock().unwrap().iter().any(|c| c == id))
                    {
                        continue;
                    }
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
    run_with_observer(world, adapter, options, confirm, &FakeObserver::default()).await
}

async fn run_with_observer(
    world: &World,
    adapter: Arc<FakeAdapter>,
    options: RunOptions,
    confirm: bool,
    observer: &dyn ObserverFactory,
) -> Result<RunSummary, String> {
    run_on_nodes(world, adapter, options, confirm, observer, "regtest").await
}

async fn run_on_nodes(
    world: &World,
    adapter: Arc<FakeAdapter>,
    options: RunOptions,
    confirm: bool,
    observer: &dyn ObserverFactory,
    network: &'static str,
) -> Result<RunSummary, String> {
    let clients = FakeClients(adapter.flags.clone());
    let docker = Arc::new(FakeDocker::default());
    let env = env_for(world, confirm);
    let wallets = nodes_on(Arc::clone(&adapter.ledger), network);
    let deps = RunDeps {
        desktop_factory: &clients,
        net: Some(&ReadyNet),
        docker: Arc::clone(&docker) as Arc<dyn ContainerRuntime>,
        probe: Arc::new(AlwaysReady),
        adapter: adapter as Arc<dyn AppAdapter>,
        env: &env,
        wallets: &wallets,
        observer,
    };
    let result = run_scenarios(&world.config, &options, &deps).await;
    assert!(
        docker.running.lock().unwrap().is_empty(),
        "a run never starts containers of its own"
    );
    result
}

fn opts_in(world: &World, names: &[&str]) -> RunOptions {
    RunOptions {
        recovery_isolation: None,
        desktop: desktop_options(world),
        stack: Some(stack()),
        stack_dir: None,
        scenarios: names.iter().map(|s| (*s).to_owned()).collect(),
        tag: None,
    }
}

mod behavior;
mod desktop;
mod evidence;
mod isolation;
mod recovery;

mod phase5;
