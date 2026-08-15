#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::time::Duration;

use mortsom_android_driver::avd::Actor;
use mortsom_android_driver::ui::tests_support::ScriptedUi as FakeUi;
use mortsom_common::Lane;

use super::*;
use mortsom_app_contract::OrderState;

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
async fn observing_an_unknown_order_is_a_contract_failure() {
    let ui = fresh_app();
    adapter().onboard(&device(&ui), &env()).await.unwrap();
    ui.present(&[Ids::NAV_TRADES]);
    let err = adapter()
        .observe_order(&device(&ui), &OrderReference::new("o"))
        .await
        .unwrap_err();
    assert_eq!(
        err.category,
        mortsom_common::FailureCategory::AutomationContract
    );
}

/// Extends the onboarded fake with a trade lifecycle: creating an order
/// makes `trades.item.o1` appear; the detail shows a status label that the
/// test advances by hand.
fn app_with_orders() -> FakeUi {
    let ui = fresh_app();
    ui.on_tap(
        Ids::ORDER_ADD_FAB,
        &[Ids::ORDER_ADD_SELL, Ids::ORDER_ADD_BUY],
    );
    ui.on_tap(
        Ids::ORDER_ADD_SELL,
        &[
            Ids::ORDER_CREATE_CURRENCY,
            Ids::ORDER_CREATE_FIAT_AMOUNT,
            Ids::ORDER_CREATE_PAYMENT_METHOD,
            Ids::ORDER_CREATE_PRICE_TYPE,
            Ids::ORDER_CREATE_SUBMIT,
        ],
    );
    ui.on_tap(Ids::ORDER_CREATE_CURRENCY, &["order.create.currency.USD"]);
    ui.on_tap(
        Ids::ORDER_CREATE_PRICE_TYPE,
        &[Ids::ORDER_CREATE_SATS_AMOUNT],
    );
    ui.on_tap(Ids::ORDER_CREATE_SUBMIT, &[Ids::ORDER_CONFIRM_HOME]);
    ui.on_tap(
        Ids::ORDER_CONFIRM_HOME,
        &["trades.item.o1", Ids::NAV_TRADES, Ids::NAV_ORDER_BOOK],
    );
    ui.on_tap(
        "trades.item.o1",
        &[
            Ids::ORDER_STATUS,
            Ids::ORDER_ID,
            Ids::APPBAR_BACK,
            Ids::TRADE_CANCEL,
            Ids::TRADE_PAY_INVOICE,
            Ids::TRADE_ADD_INVOICE,
            Ids::TRADE_FIAT_SENT,
            Ids::TRADE_RELEASE,
        ],
    );
    ui.on_tap(
        Ids::TRADE_PAY_INVOICE,
        &[Ids::PAY_INVOICE_TEXT, Ids::PAY_NWC],
    );
    ui.on_tap(Ids::TRADE_ADD_INVOICE, &[Ids::INVOICE_NWC_GENERATE]);
    // Confirming navigates home: the detail disappears until the daemon answers.
    ui.on_tap_hide(
        Ids::INVOICE_NWC_CONFIRM,
        &[Ids::ORDER_STATUS, Ids::INVOICE_NWC_CONFIRM],
    );
    ui.on_tap_hide(Ids::PAY_NWC, &[Ids::PAY_NWC, Ids::PAY_INVOICE_TEXT]);
    ui.on_tap(
        Ids::INVOICE_NWC_GENERATE,
        &[Ids::INVOICE_NWC_TEXT, Ids::INVOICE_NWC_CONFIRM],
    );
    ui.on_tap(Ids::TRADE_RELEASE, &[Ids::TRADE_RELEASE_CONFIRM]);
    ui.on_tap(Ids::TRADE_CANCEL, &[Ids::TRADE_CANCEL_CONFIRM]);
    ui.on_tap(Ids::ORDER_BOOK_TAB_SELL, &["order.book.item.o1"]);
    ui.on_tap("order.book.item.o1", &[Ids::ORDER_TAKE_CONFIRM]);
    ui.on_tap(Ids::ORDER_TAKE_CONFIRM, &[Ids::ORDER_STATUS]);
    ui.present(&[
        Ids::NAV_TRADES,
        Ids::NAV_ORDER_BOOK,
        Ids::ORDER_BOOK_TAB_SELL,
    ]);
    ui.label(Ids::ORDER_STATUS, "pending");
    ui.label(Ids::ORDER_ID, "o1");
    ui.label(Ids::PAY_INVOICE_TEXT, "lnbc1000n1secretinvoice");
    ui.label(Ids::INVOICE_NWC_TEXT, "lnbc1000n1buyerinvoice");
    ui
}

fn draft() -> OrderDraft {
    OrderDraft {
        kind: mortsom_app_contract::OrderKind::Sell,
        amount_sats: 1000,
        fiat_code: "USD".into(),
        fiat_amount: 1,
        payment_method: "cash".into(),
    }
}

#[tokio::test]
async fn create_order_returns_the_new_trade_id_and_state() {
    let ui = app_with_orders();
    adapter().onboard(&device(&ui), &env()).await.unwrap();

    let order = adapter()
        .create_order(&device(&ui), &draft())
        .await
        .unwrap();

    assert_eq!(order.id.as_deref(), Some("o1"));
    assert_eq!(order.state, OrderState::Pending);
    let calls = ui.calls();
    assert!(calls.contains(&"tap order.create.currency.USD".to_owned()));
    assert!(calls.contains(&"type order.create.sats_amount (4 chars)".to_owned()));
    assert!(
        calls.contains(&"tap order.create.price_type".to_owned()),
        "market -> fixed"
    );
}

#[tokio::test]
async fn trade_actions_drive_the_expected_controls_and_never_record_invoices() {
    let ui = app_with_orders();
    adapter().onboard(&device(&ui), &env()).await.unwrap();
    let order = OrderReference::with_kind("o1", mortsom_app_contract::OrderKind::Sell);
    adapter()
        .create_order(&device(&ui), &draft())
        .await
        .unwrap();

    adapter()
        .find_and_take_order(&device(&ui), &order)
        .await
        .unwrap();
    let handoff = adapter()
        .pay_hold_invoice(&device(&ui), &order)
        .await
        .unwrap();
    let buyer = adapter()
        .provide_buyer_invoice(&device(&ui), &order)
        .await
        .unwrap();
    adapter()
        .confirm_fiat_sent(&device(&ui), &order)
        .await
        .unwrap();
    adapter().release(&device(&ui), &order).await.unwrap();

    assert_eq!(handoff.kind, mortsom_app_contract::PaymentPath::Nwc);
    assert_eq!(
        handoff.invoice.as_ref().unwrap().expose(),
        "lnbc1000n1secretinvoice"
    );
    assert_eq!(buyer.unwrap().expose(), "lnbc1000n1buyerinvoice");
    assert!(!format!("{handoff:?}").contains("secretinvoice"));
    let calls = ui.calls();
    for expected in [
        "tap order.book.tab.sell",
        "tap order.book.item.o1",
        "tap order.take.confirm",
        "tap trade.payInvoice",
        "tap pay.nwc",
        "tap invoice.nwc.generate",
        "tap invoice.nwc.confirm",
        "tap trade.fiatSent",
        "tap trade.release",
        "tap trade.release.confirm",
    ] {
        assert!(calls.contains(&expected.to_owned()), "missing {expected}");
    }
}

#[tokio::test]
async fn cancel_if_open_is_idempotent_and_observe_maps_statuses() {
    let ui = app_with_orders();
    adapter().onboard(&device(&ui), &env()).await.unwrap();
    let order = OrderReference::new("o1");
    assert_eq!(
        adapter()
            .cancel_if_open(&device(&ui), &order)
            .await
            .unwrap(),
        CancelOutcome::NotFound
    );

    adapter()
        .create_order(&device(&ui), &draft())
        .await
        .unwrap();
    assert_eq!(
        adapter()
            .cancel_if_open(&device(&ui), &order)
            .await
            .unwrap(),
        CancelOutcome::Cancelled
    );

    ui.label(Ids::ORDER_STATUS, "canceled");
    assert_eq!(
        adapter()
            .cancel_if_open(&device(&ui), &order)
            .await
            .unwrap(),
        CancelOutcome::AlreadyClosed
    );
    let observed = adapter().observe_order(&device(&ui), &order).await.unwrap();
    assert_eq!(observed.state, OrderState::Cancelled);
    assert_eq!(observed.raw_label, "canceled");

    ui.label(Ids::ORDER_STATUS, "weird");
    let err = adapter()
        .observe_order(&device(&ui), &order)
        .await
        .unwrap_err();
    assert_eq!(err.category, mortsom_common::FailureCategory::Product);
}
