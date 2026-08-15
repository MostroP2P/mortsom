#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::BTreeMap;
use std::sync::Mutex;
use std::time::Duration;

use mortsom_android_driver::appium::AppiumError;
use mortsom_android_driver::avd::Actor;
use mortsom_android_driver::ui::UiSession;
use mortsom_common::Lane;

use super::*;

/// Scripted UI: elements exist when listed (dynamic changes via `on_tap`),
/// labels answer from a map, and every action is recorded.
#[derive(Default)]
struct FakeUi {
    present: Mutex<Vec<String>>,
    labels: Mutex<BTreeMap<String, String>>,
    calls: Mutex<Vec<String>>,
    on_tap: Mutex<BTreeMap<String, Vec<String>>>,
}

impl FakeUi {
    fn present(&self, ids: &[&str]) {
        self.present
            .lock()
            .unwrap()
            .extend(ids.iter().map(|s| (*s).to_owned()));
    }
    fn label(&self, id: &str, value: &str) {
        self.labels.lock().unwrap().insert(id.into(), value.into());
    }
    fn on_tap(&self, id: &str, appear: &[&str]) {
        self.on_tap
            .lock()
            .unwrap()
            .insert(id.into(), appear.iter().map(|s| (*s).to_owned()).collect());
    }
    fn calls(&self) -> Vec<String> {
        self.calls.lock().unwrap().clone()
    }
    fn has(&self, id: &str) -> bool {
        self.present.lock().unwrap().iter().any(|p| p == id)
    }
}

#[async_trait]
impl UiSession for FakeUi {
    async fn exists(&self, id: &str) -> Result<bool, AppiumError> {
        Ok(self.has(id))
    }
    async fn wait_for(&self, id: &str, _: Duration) -> Result<(), AppiumError> {
        if self.has(id) {
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
        self.calls.lock().unwrap().push(format!("tap {id}"));
        if let Some(appear) = self.on_tap.lock().unwrap().get(id).cloned() {
            self.present.lock().unwrap().extend(appear);
        }
        Ok(())
    }
    async fn type_text(&self, id: &str, text: &str) -> Result<(), AppiumError> {
        // Record the length only: secrets never reach logs, not even fakes'.
        self.calls
            .lock()
            .unwrap()
            .push(format!("type {id} ({} chars)", text.len()));
        Ok(())
    }
    async fn text_of(&self, _: &str) -> Result<String, AppiumError> {
        Ok(String::new())
    }
    async fn is_enabled(&self, _: &str) -> Result<bool, AppiumError> {
        Ok(true)
    }
    async fn label_of(&self, id: &str) -> Result<String, AppiumError> {
        Ok(self
            .labels
            .lock()
            .unwrap()
            .get(id)
            .cloned()
            .unwrap_or_default())
    }
    async fn screenshot_png(&self) -> Result<Vec<u8>, AppiumError> {
        Ok(vec![])
    }
    async fn page_source(&self) -> Result<String, AppiumError> {
        Ok(String::new())
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
    async fn end(&self) -> Result<(), AppiumError> {
        Ok(())
    }
}

const PUBKEY: &str = "8171eb680049f62ce8747d203f57c3f71de6eea308af6f995363896a1e47cc51";
const RELAY_ROW: &str = "settings.relays.item.ws://10.0.2.2:7000";

fn env() -> TestEnvironment {
    TestEnvironment {
        mostro_pubkey: PUBKEY.into(),
        relay_url: "ws://10.0.2.2:7000".into(),
        lane: Lane::Local,
    }
}

fn adapter() -> MobileLegacyAdapter {
    MobileLegacyAdapter::new(
        "network.mostro.app",
        PathBuf::from("/bin/flutter"),
        "ws://10.0.2.2:7000",
    )
}

fn device(ui: &FakeUi) -> DeviceContext<'_> {
    DeviceContext {
        actor: Actor::Alice,
        ui,
        ui_timeout: Duration::from_secs(1),
    }
}

/// A fake app in its onboarding state that transitions like the real one.
fn fresh_app() -> FakeUi {
    let ui = FakeUi::default();
    ui.present(&[Ids::ONBOARDING_SKIP]);
    ui.on_tap(
        Ids::ONBOARDING_SKIP,
        &[Ids::COMMUNITY_CUSTOM_NODE, Ids::COMMUNITY_SKIP],
    );
    ui.on_tap(
        Ids::COMMUNITY_CUSTOM_NODE,
        &[
            Ids::NODE_CUSTOM_PUBKEY,
            Ids::NODE_CUSTOM_NAME,
            Ids::NODE_CUSTOM_CONFIRM,
        ],
    );
    ui.on_tap(Ids::NODE_CUSTOM_CONFIRM, &[Ids::COMMUNITY_DONE]);
    ui.on_tap(
        Ids::COMMUNITY_DONE,
        &[Ids::ORDER_ADD_FAB, Ids::ENV_MARKER, Ids::APPBAR_DRAWER],
    );
    ui.on_tap(
        Ids::APPBAR_DRAWER,
        &[Ids::DRAWER_ACCOUNT, Ids::DRAWER_SETTINGS],
    );
    ui.on_tap(
        Ids::DRAWER_ACCOUNT,
        &[Ids::KEYS_PUBLIC_KEY, Ids::APPBAR_BACK],
    );
    ui.on_tap(
        Ids::DRAWER_SETTINGS,
        &[
            Ids::SETTINGS_MOSTRO_NODE,
            Ids::SETTINGS_MOSTRO_NODE_PUBKEY,
            Ids::SETTINGS_RELAYS_ADD,
            Ids::SETTINGS_WALLET,
            Ids::APPBAR_BACK,
        ],
    );
    ui.on_tap(
        Ids::SETTINGS_RELAYS_ADD,
        &[
            Ids::SETTINGS_RELAYS_ADD_URL,
            Ids::SETTINGS_RELAYS_ADD_CONFIRM,
        ],
    );
    ui.on_tap(Ids::SETTINGS_RELAYS_ADD_CONFIRM, &[RELAY_ROW]);
    ui.on_tap(Ids::SETTINGS_WALLET, &[Ids::WALLET_SETTINGS_CONNECT]);
    ui.on_tap(
        Ids::WALLET_SETTINGS_CONNECT,
        &[Ids::WALLET_NWC_URI, Ids::WALLET_NWC_CONNECT],
    );
    ui.on_tap(Ids::WALLET_NWC_CONNECT, &[Ids::WALLET_CONNECTION]);
    ui.label(Ids::KEYS_PUBLIC_KEY, "npub1alice");
    ui.label(Ids::SETTINGS_MOSTRO_NODE_PUBKEY, PUBKEY);
    ui.label(Ids::WALLET_CONNECTION, "connected");
    ui
}

#[tokio::test]
async fn onboarding_selects_the_daemon_and_reads_the_identity() {
    let ui = fresh_app();
    let identity = adapter().onboard(&device(&ui), &env()).await.unwrap();

    assert_eq!(identity.public_key, "npub1alice");
    let calls = ui.calls();
    assert!(calls.contains(&"tap onboarding.walkthrough.skip".to_owned()));
    assert!(calls.contains(&format!("type node.custom.pubkey ({} chars)", PUBKEY.len())));
    assert!(calls.contains(&"tap onboarding.community.done".to_owned()));
    assert!(calls.contains(&"tap drawer.account".to_owned()));
}

#[tokio::test]
async fn onboarding_refuses_a_build_without_the_environment_marker() {
    let ui = fresh_app();
    ui.on_tap(Ids::COMMUNITY_DONE, &[Ids::ORDER_ADD_FAB]);

    let err = adapter().onboard(&device(&ui), &env()).await.unwrap_err();

    assert!(err.message.contains("marker"));
}

#[tokio::test]
async fn node_and_relay_configuration_adds_the_relay_when_missing() {
    let ui = fresh_app();
    adapter().onboard(&device(&ui), &env()).await.unwrap();

    adapter()
        .configure_node_and_relay(&device(&ui), &env())
        .await
        .unwrap();

    let calls = ui.calls();
    assert!(calls.contains(&"tap settings.relays.add".to_owned()));
    assert!(calls.contains(&"type settings.relays.add.url (18 chars)".to_owned()));
    assert!(ui.has(RELAY_ROW));
}

#[tokio::test]
async fn node_mismatch_is_a_product_failure_and_seeded_relay_skips_the_dialog() {
    let ui = fresh_app();
    ui.label(Ids::SETTINGS_MOSTRO_NODE_PUBKEY, "someone-else");
    adapter().onboard(&device(&ui), &env()).await.unwrap();
    let err = adapter()
        .configure_node_and_relay(&device(&ui), &env())
        .await
        .unwrap_err();
    assert_eq!(err.category, mortsom_common::FailureCategory::Product);

    let ui = fresh_app();
    ui.present(&[RELAY_ROW]);
    adapter().onboard(&device(&ui), &env()).await.unwrap();
    adapter()
        .configure_node_and_relay(&device(&ui), &env())
        .await
        .unwrap();
    assert!(!ui.calls().contains(&"tap settings.relays.add".to_owned()));
}

#[tokio::test]
async fn nwc_setup_types_the_uri_without_recording_it_and_reads_connected() {
    let ui = fresh_app();
    adapter().onboard(&device(&ui), &env()).await.unwrap();
    let uri = Secret::new("nostr+walletconnect://abc?relay=wss%3A%2F%2Fr&secret=deadbeef");

    let connection = adapter()
        .configure_nwc(&device(&ui), "alice", &uri)
        .await
        .unwrap();

    assert!(connection.connected);
    assert_eq!(connection.label, "alice");
    let calls = ui.calls().join("\n");
    assert!(calls.contains("type wallet.nwc.uri"));
    assert!(!calls.contains("deadbeef"));
    assert!(!format!("{connection:?}").contains("deadbeef"));
}

#[tokio::test]
async fn missing_identifier_is_an_automation_contract_failure() {
    let ui = FakeUi::default();
    let err = adapter().onboard(&device(&ui), &env()).await.unwrap_err();
    assert_eq!(
        err.category,
        mortsom_common::FailureCategory::AutomationContract
    );
    assert!(err.message.contains(Ids::ONBOARDING_SKIP));
}

#[test]
fn profile_and_capabilities_cover_the_happy_sell_requirements() {
    let a = adapter();
    assert_eq!(a.profile().name, PROFILE);
    let required = [
        Capability::Onboarding,
        Capability::CustomNodeSelection,
        Capability::LocalRelayConfiguration,
        Capability::Nwc,
        Capability::CreateOrder,
        Capability::TakeOrder,
        Capability::HoldInvoicePayment,
        Capability::FiatSent,
        Capability::Release,
        Capability::CompletedObservation,
    ];
    assert!(a.capabilities().missing(&required).is_empty());
}

#[tokio::test]
async fn order_journeys_are_reported_as_pending_phase_four() {
    let ui = FakeUi::default();
    let order = OrderReference { id: "o".into() };
    let err = adapter()
        .observe_order(&device(&ui), &order)
        .await
        .unwrap_err();
    assert!(err.message.contains("Phase 4"));
}

#[tokio::test]
async fn an_app_without_an_identity_gets_one_generated_through_the_ui() {
    let ui = fresh_app();
    ui.label(Ids::KEYS_PUBLIC_KEY, "");
    ui.on_tap(
        Ids::DRAWER_ACCOUNT,
        &[Ids::KEYS_PUBLIC_KEY, Ids::KEYS_GENERATE, Ids::APPBAR_BACK],
    );
    ui.on_tap(Ids::KEYS_GENERATE, &[Ids::KEYS_GENERATE_CONFIRM]);

    let result = adapter().onboard(&device(&ui), &env()).await;

    let calls = ui.calls();
    assert!(calls.contains(&"tap keys.generate".to_owned()));
    assert!(calls.contains(&"tap keys.generate.confirm".to_owned()));
    // The fake never populates the key, which the adapter reports as a
    // product failure rather than inventing an identity.
    let err = result.unwrap_err();
    assert_eq!(err.category, mortsom_common::FailureCategory::Product);
}
