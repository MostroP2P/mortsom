#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::time::Duration;

use mortsom_android_driver::avd::Actor;
use mortsom_android_driver::ui::tests_support::ScriptedUi as FakeUi;
use mortsom_app_contract::{OrderState, PaymentPath};
use mortsom_common::Lane;

use super::*;

const PUBKEY: &str = "8171eb680049f62ce8747d203f57c3f71de6eea308af6f995363896a1e47cc51";
const RELAY_ROW: &str = "settings.relays.item.ws://10.0.2.2:7000";

fn env() -> TestEnvironment {
    TestEnvironment {
        mostro_pubkey: PUBKEY.into(),
        relay_url: "ws://10.0.2.2:7000".into(),
        lane: Lane::Local,
    }
}

fn adapter() -> MobileV2Adapter {
    MobileV2Adapter::new(
        DEFAULT_PACKAGE,
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

/// A fake v2 app on first launch that transitions like the real one: the
/// walkthrough lands on the book with the marker; the drawer reaches the
/// account (key generated on first launch) and settings (node selector,
/// relays route, wallet route).
fn fresh_app() -> FakeUi {
    let ui = FakeUi::default();
    ui.present(&[Ids::WALKTHROUGH_SKIP]);
    ui.on_tap(
        Ids::WALKTHROUGH_SKIP,
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
            Ids::SETTINGS_RELAYS,
            Ids::SETTINGS_WALLET,
            Ids::APPBAR_BACK,
        ],
    );
    ui.on_tap(
        Ids::SETTINGS_MOSTRO_NODE,
        &[
            Ids::NODE_CUSTOM_PUBKEY,
            Ids::NODE_CUSTOM_CONFIRM,
            Ids::NODE_USE_DEFAULT,
        ],
    );
    ui.on_tap(Ids::SETTINGS_RELAYS, &[Ids::SETTINGS_RELAYS_ADD]);
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
    ui.label(
        Ids::SETTINGS_MOSTRO_NODE_PUBKEY,
        "82fa8cb978b43c79b2156585bac2c011176a21d2aead6d9f7c575c005be88390",
    );
    ui.label(Ids::WALLET_CONNECTION, wallet::CONNECTED);
    ui
}

/// The walkthrough's first frame is not up the instant the session opens:
/// a cold start draws it a moment later. Asking once and moving on read an
/// app that was still starting as an app with no walkthrough, and then
/// waited out the whole UI timeout on the slide the order book never
/// replaces -- which is what a real run reported as `order.add.fab did not
/// appear`, with the skip button sitting in the hierarchy beside it.
#[tokio::test]
async fn a_walkthrough_that_is_still_drawing_is_waited_for_not_missed() {
    let ui = fresh_app();
    ui.appears_after(Ids::WALKTHROUGH_SKIP, 2);

    let identity = adapter().onboard(&device(&ui), &env()).await.unwrap();

    assert_eq!(identity.public_key, "npub1alice");
    assert_eq!(
        ui.calls().first().map(String::as_str),
        Some("tap onboarding.walkthrough.skip"),
        "the walkthrough was skipped, not waited out: {:?}",
        ui.calls()
    );
}

/// An app stuck on its splash screen shows none of the screens onboarding
/// waits for. That wait has already spent the UI timeout by the time it
/// gives up, so reaching for the order book again would double how long a
/// stuck actor takes to report -- and every actor's bootstrap is
/// sequential, with the failure evidence and the cleanup queued behind it.
#[tokio::test]
async fn an_app_that_shows_nothing_is_reported_once_not_waited_for_twice() {
    // A launch that draws neither the walkthrough nor the book.
    let ui = FakeUi::default();

    let error = adapter().onboard(&device(&ui), &env()).await.unwrap_err();

    assert_eq!(
        error.category,
        mortsom_common::FailureCategory::AutomationContract
    );
    assert!(
        error
            .message
            .contains("neither the walkthrough nor the order book"),
        "the report should name what the app failed to show: {}",
        error.message
    );
}

#[tokio::test]
async fn onboarding_skips_the_walkthrough_and_reads_the_generated_identity() {
    let ui = fresh_app();
    let identity = adapter().onboard(&device(&ui), &env()).await.unwrap();
    assert_eq!(identity.public_key, "npub1alice");
    let calls = ui.calls();
    assert_eq!(calls[0], "tap onboarding.walkthrough.skip");
    assert!(calls.contains(&"tap drawer.account".to_owned()));
    assert!(
        !calls.contains(&"tap keys.generate".to_owned()),
        "key already generated"
    );
}

#[tokio::test]
async fn a_build_without_the_marker_is_refused_as_the_wrong_artifact() {
    let ui = FakeUi::default();
    ui.present(&[Ids::WALKTHROUGH_SKIP]);
    ui.on_tap(Ids::WALKTHROUGH_SKIP, &[Ids::ORDER_ADD_FAB]);
    let err = adapter().onboard(&device(&ui), &env()).await.unwrap_err();
    // A missing marker means the installed APK is not the build under
    // test; that is a prerequisite problem, not the app breaking a promise.
    assert_eq!(
        err.category,
        mortsom_common::FailureCategory::EnvironmentMisconfiguration
    );
    assert!(err.message.contains("marker"));
}

#[tokio::test]
async fn node_selection_uses_the_dialog_only_when_the_default_is_shown() {
    let ui = fresh_app();
    adapter().onboard(&device(&ui), &env()).await.unwrap();
    // Confirming the dialog swaps the shown pubkey for the daemon under test.
    ui.on_tap_relabel(
        Ids::NODE_CUSTOM_CONFIRM,
        Ids::SETTINGS_MOSTRO_NODE_PUBKEY,
        PUBKEY,
    );
    adapter()
        .configure_node_and_relay(&device(&ui), &env())
        .await
        .unwrap();
    let calls = ui.calls();
    assert!(calls.contains(&"tap settings.mostro_node".to_owned()));
    assert!(calls.contains(&format!("type node.custom.pubkey ({} chars)", PUBKEY.len())));
    assert!(calls.contains(&"tap settings.relays.add".to_owned()));

    // Second time: node already selected, relay already listed.
    let again = ui.calls().len();
    adapter()
        .configure_node_and_relay(&device(&ui), &env())
        .await
        .unwrap();
    let new_calls = &ui.calls()[again..];
    assert!(!new_calls.contains(&"tap settings.mostro_node".to_owned()));
    assert!(!new_calls.contains(&"tap settings.relays.add".to_owned()));
}

#[tokio::test]
async fn nwc_setup_types_the_uri_without_recording_it() {
    let ui = fresh_app();
    adapter().onboard(&device(&ui), &env()).await.unwrap();
    let uri = Secret::new("nostr+walletconnect://abc?relay=wss%3A%2F%2Fr&secret=deadbeef");
    let connection = adapter()
        .configure_nwc(&device(&ui), "bob", &uri)
        .await
        .unwrap();
    assert!(connection.connected);
    let calls = ui.calls().join("\n");
    assert!(calls.contains("type wallet.nwc.uri"));
    assert!(!calls.contains("deadbeef"));
}

#[test]
fn profile_and_capabilities_cover_the_happy_sell_requirements() {
    let a = adapter();
    assert_eq!(a.profile().name, PROFILE);
    assert_eq!(a.profile().package, DEFAULT_PACKAGE);
    let required = [
        Capability::Onboarding,
        Capability::CustomNodeSelection,
        Capability::LocalRelayConfiguration,
        Capability::Nwc,
        Capability::CreateOrder,
        Capability::TakeOrder,
        Capability::HoldInvoicePayment,
        Capability::BuyerInvoice,
        Capability::FiatSent,
        Capability::Release,
        Capability::CompletedObservation,
    ];
    assert!(a.capabilities().missing(&required).is_empty());
    assert!(
        !a.capabilities()
            .missing(&[Capability::Notifications])
            .is_empty()
    );
}

/// Extends the onboarded fake with a trade lifecycle.
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
    ui.on_tap(
        Ids::ORDER_CREATE_CURRENCY,
        &[
            Ids::ORDER_CREATE_CURRENCY_SEARCH,
            "order.create.currency.USD",
        ],
    );
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
        &[Ids::PAY_INVOICE_TEXT, Ids::PAY_NWC, Ids::PAY_CANCEL],
    );
    // With a wallet connected v2 generates and submits the buyer invoice in
    // one step; the readout is all the screen leaves behind.
    ui.on_tap(Ids::TRADE_ADD_INVOICE, &[Ids::INVOICE_NWC_TEXT]);
    ui.on_tap_hide(Ids::PAY_NWC, &[Ids::PAY_NWC, Ids::PAY_INVOICE_TEXT]);
    ui.on_tap(Ids::TRADE_RELEASE, &[Ids::TRADE_RELEASE_CONFIRM]);
    ui.on_tap(Ids::TRADE_CANCEL, &[Ids::TRADE_CANCEL_CONFIRM]);
    // A sell order is listed on the "Buy BTC" tab: the taker buys.
    ui.on_tap(Ids::ORDER_BOOK_TAB_BUY, &["order.book.item.o1"]);
    ui.on_tap("order.book.item.o1", &[Ids::ORDER_TAKE_CONFIRM]);
    ui.on_tap(Ids::ORDER_TAKE_CONFIRM, &[Ids::ORDER_STATUS]);
    ui.present(&[
        Ids::NAV_TRADES,
        Ids::NAV_ORDER_BOOK,
        Ids::ORDER_BOOK_TAB_BUY,
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
        kind: OrderKind::Sell,
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
}

#[tokio::test]
async fn trade_actions_drive_the_v2_controls_and_map_v2_states() {
    let ui = app_with_orders();
    let a = adapter();
    a.onboard(&device(&ui), &env()).await.unwrap();
    a.create_order(&device(&ui), &draft()).await.unwrap();
    let order = OrderReference::with_kind("o1", OrderKind::Sell);

    a.find_and_take_order(&device(&ui), &order).await.unwrap();
    ui.label(Ids::ORDER_STATUS, "waiting-payment");
    let state = a.observe_order(&device(&ui), &order).await.unwrap();
    assert_eq!(state.state, OrderState::Taken);
    assert_eq!(state.raw_label, "waiting-payment");

    let invoice = a
        .provide_buyer_invoice(&device(&ui), &order, None)
        .await
        .unwrap();
    assert!(invoice.is_some());
    let handoff = a
        .pay_hold_invoice(&device(&ui), &order, PaymentPath::Nwc)
        .await
        .unwrap();
    assert_eq!(handoff.kind, PaymentPath::Nwc);
    assert!(handoff.invoice.is_some());
    ui.label(Ids::ORDER_STATUS, "active");
    let actions = a.visible_actions(&device(&ui), &order).await.unwrap();
    assert!(actions.contains(&"fiat-sent".to_owned()));
    a.confirm_fiat_sent(&device(&ui), &order).await.unwrap();
    ui.label(Ids::ORDER_STATUS, "fiat-sent");
    a.release(&device(&ui), &order).await.unwrap();
    ui.label(Ids::ORDER_STATUS, "pending-rating");
    assert_eq!(
        a.observe_order(&device(&ui), &order).await.unwrap().state,
        OrderState::Completed
    );
    assert_eq!(
        a.cancel_if_open(&device(&ui), &order).await.unwrap(),
        CancelOutcome::AlreadyClosed
    );
    let calls = ui.calls().join("\n");
    assert!(calls.contains("tap trade.release.confirm"));
    assert!(!calls.contains("secretinvoice") && !calls.contains("buyerinvoice"));
}

#[tokio::test]
async fn loading_status_keeps_the_caller_waiting_and_cancel_paths_work() {
    let ui = app_with_orders();
    let a = adapter();
    a.onboard(&device(&ui), &env()).await.unwrap();
    a.create_order(&device(&ui), &draft()).await.unwrap();
    let order = OrderReference::new("o1");
    ui.label(Ids::ORDER_STATUS, "loading");
    let err = a.observe_order(&device(&ui), &order).await.unwrap_err();
    assert!(err.message.contains("unresolved status `loading`"));
    ui.label(Ids::ORDER_STATUS, "pending");
    a.cancel_payment(&device(&ui), &order).await.unwrap();
    assert!(ui.calls().contains(&"tap pay.cancel".to_owned()));
    assert_eq!(
        a.cancel_if_open(&device(&ui), &order).await.unwrap(),
        CancelOutcome::Cancelled
    );
    assert_eq!(
        a.cancel_if_open(&device(&ui), &OrderReference::new("nope"))
            .await
            .unwrap(),
        CancelOutcome::NotFound
    );
}

/// A run whose wallet is a node Mortsom holds: the app has none connected,
/// so it shows the hold invoice and waits, and accepts a buyer invoice
/// typed into its manual form.
fn app_without_a_wallet() -> FakeUi {
    let ui = app_with_orders();
    // No connected wallet: the add-invoice screen opens on the manual form
    // and the pay screen offers no auto-pay button.
    ui.on_tap(
        Ids::TRADE_ADD_INVOICE,
        &[Ids::INVOICE_TEXT, Ids::INVOICE_SUBMIT],
    );
    ui.on_tap(Ids::INVOICE_SUBMIT, &[Ids::ORDER_ADD_FAB]);
    ui
}

#[tokio::test]
async fn the_hold_invoice_is_handed_over_without_the_app_paying_it() {
    let ui = app_without_a_wallet();
    let a = adapter();
    a.onboard(&device(&ui), &env()).await.unwrap();
    a.create_order(&device(&ui), &draft()).await.unwrap();
    let order = OrderReference::new("o1");

    let handoff = a
        .pay_hold_invoice(&device(&ui), &order, PaymentPath::ExternalWallet)
        .await
        .unwrap();

    assert_eq!(handoff.kind, PaymentPath::ExternalWallet);
    // The caller pays it, so it has to come back.
    assert!(handoff.invoice.is_some());
    let calls = ui.calls().join("\n");
    assert!(
        !calls.contains("tap pay.nwc"),
        "the app has no wallet to pay with",
    );
    // The invoice is a payable instrument until it settles; it never
    // reaches the call log.
    assert!(!calls.contains("secretinvoice"), "{calls}");
}

/// An empty pay screen means there is nothing to hand over. Returning a
/// handoff with no invoice would have the caller pay nothing and the run
/// wait for a settlement that cannot arrive.
#[tokio::test]
async fn an_empty_pay_screen_is_a_product_failure_not_an_empty_handoff() {
    let ui = app_without_a_wallet();
    let a = adapter();
    a.onboard(&device(&ui), &env()).await.unwrap();
    a.create_order(&device(&ui), &draft()).await.unwrap();
    ui.label(Ids::PAY_INVOICE_TEXT, "   ");

    let err = a
        .pay_hold_invoice(
            &device(&ui),
            &OrderReference::new("o1"),
            PaymentPath::ExternalWallet,
        )
        .await
        .unwrap_err();

    assert_eq!(err.category, mortsom_common::FailureCategory::Product);
    assert!(err.message.contains("nothing to pay"), "{err}");
}

#[tokio::test]
async fn a_supplied_buyer_invoice_is_typed_into_the_manual_form() {
    let ui = app_without_a_wallet();
    let a = adapter();
    a.onboard(&device(&ui), &env()).await.unwrap();
    a.create_order(&device(&ui), &draft()).await.unwrap();
    let supplied = Secret::new("lnbc1000n1mortsominvoice");

    let returned = a
        .provide_buyer_invoice(&device(&ui), &OrderReference::new("o1"), Some(&supplied))
        .await
        .unwrap();

    assert_eq!(
        returned.as_ref().map(|s| s.expose()),
        Some(supplied.expose()),
        "the caller correlates the payment against the invoice it issued",
    );
    let calls = ui.calls().join("\n");
    assert!(calls.contains(&format!(
        "type invoice.text ({} chars)",
        supplied.expose().len()
    )));
    assert!(calls.contains("tap invoice.submit"));
    assert!(!calls.contains("mortsominvoice"), "{calls}");
}
