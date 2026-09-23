#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::time::Duration;

use mortsom_app_contract::{OrderState, PaymentPath};
use mortsom_common::Actor;
use mortsom_common::Lane;
use mortsom_ui_driver::ui::tests_support::ScriptedUi as FakeUi;

use super::*;

const PUBKEY: &str = "8171eb680049f62ce8747d203f57c3f71de6eea308af6f995363896a1e47cc51";
const RELAY_ROW: &str = "settings.relays.item.ws://127.0.0.1:7000";

fn env() -> TestEnvironment {
    TestEnvironment {
        mostro_pubkey: PUBKEY.into(),
        relay_url: "ws://127.0.0.1:7000".into(),
        lane: Lane::Local,
    }
}

fn adapter() -> MobileV2Adapter {
    MobileV2Adapter::new()
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

/// The last field of the order form is a number, so typing into it leaves
/// the numeric keyboard up -- over the bottom of the form, where submit
/// lives. A keyboard is not something the driver can look underneath: it
/// shrinks the viewport and the app's semantics stop there, so scrolling
/// finds nothing either. A real Polar run died exactly here, polling
/// `order.create.submit` for its whole 30s with the form correctly filled
/// in behind the keyboard.
#[tokio::test]
async fn the_keyboard_is_put_away_before_the_order_is_submitted() {
    let ui = app_with_orders();
    adapter().onboard(&device(&ui), &env()).await.unwrap();
    ui.keyboard_over(&[Ids::ORDER_CREATE_SUBMIT]);

    let order = adapter()
        .create_order(&device(&ui), &draft())
        .await
        .unwrap();

    assert_eq!(order.id.as_deref(), Some("o1"), "the order was submitted");
    let calls = ui.calls();
    let hid = calls.iter().position(|c| c == "hide keyboard");
    let submitted = calls
        .iter()
        .position(|c| c == "tap order.create.submit")
        .expect("submit was never tapped");
    assert!(
        hid.is_some_and(|h| h < submitted),
        "submit was tapped with the keyboard still up: {calls:?}"
    );
}

/// A lost acknowledgement cannot safely distinguish a dropped tap from an order
/// accepted by the daemon. Preserve uncertainty and never create a second order.
#[tokio::test]
async fn a_swallowed_submit_is_reported_without_retrying_publication() {
    let ui = app_with_orders();
    adapter().onboard(&device(&ui), &env()).await.unwrap();
    ui.swallows_taps(Ids::ORDER_CREATE_SUBMIT, 1);
    assert!(
        adapter()
            .create_order(&device(&ui), &draft())
            .await
            .is_err()
    );
    assert_eq!(
        ui.calls()
            .iter()
            .filter(|call| *call == "tap order.create.submit")
            .count(),
        1
    );
}

/// Submit is not a button that may be pressed twice: the first press
/// publishes a real order on mainnet, and a second one publishes another
/// that `new_trade_id` never sees and cleanup never cancels. Publishing
/// waits on the network, so the confirmation can take longer than the UI
/// budget while the first order is already out there -- and the app's own
/// evidence that it took the tap is that the form it submitted is gone.
#[tokio::test]
async fn a_submit_the_app_took_is_not_sent_twice() {
    let ui = app_with_orders();
    adapter().onboard(&device(&ui), &env()).await.unwrap();
    // The form is replaced the moment the app takes the submit, but the
    // confirmation only draws once the network answers -- later than the
    // wait for it.
    ui.on_tap_hide(Ids::ORDER_CREATE_SUBMIT, &[Ids::ORDER_CREATE_SUBMIT]);
    ui.appears_after(Ids::ORDER_CONFIRM_HOME, 6);

    let outcome = adapter().create_order(&device(&ui), &draft()).await;

    assert!(
        outcome.is_err(),
        "late confirmation is an uncertain outcome"
    );
    let submits = ui
        .calls()
        .iter()
        .filter(|c| *c == "tap order.create.submit")
        .count();
    assert_eq!(
        submits,
        1,
        "the order was submitted more than once: {:?}",
        ui.calls()
    );
}

/// The other shape of the same danger: the app keeps the form on screen
/// while it publishes and greys submit out. The button is still drawn, so
/// looking for it says nothing -- only that it no longer accepts a press
/// separates a form still waiting on the network from one nobody
/// submitted.
#[tokio::test]
async fn a_submit_still_working_is_waited_for_not_pressed_again() {
    let ui = app_with_orders();
    adapter().onboard(&device(&ui), &env()).await.unwrap();
    ui.disables_after_tap(Ids::ORDER_CREATE_SUBMIT);
    ui.appears_after(Ids::ORDER_CONFIRM_HOME, 6);

    let outcome = adapter().create_order(&device(&ui), &draft()).await;

    assert!(
        outcome.is_err(),
        "late confirmation is an uncertain outcome"
    );
    let submits = ui
        .calls()
        .iter()
        .filter(|c| *c == "tap order.create.submit")
        .count();
    assert_eq!(
        submits,
        1,
        "a submit the app was still working on was pressed again: {:?}",
        ui.calls()
    );
}

/// The same swallowed tap on the drawer: after a relaunch the app is still
/// settling, the hamburger is on screen, and the tap opens nothing. A real
/// run reported `drawer.account did not appear` with the order book -- and
/// the closed drawer -- in the screenshot beside it.
#[tokio::test]
async fn a_swallowed_drawer_tap_is_tapped_again() {
    let ui = app_with_orders();
    // The drawer closes behind you when you pick a destination, so its
    // items are only on screen while it is open.
    ui.on_tap_hide(
        Ids::DRAWER_ACCOUNT,
        &[Ids::DRAWER_ACCOUNT, Ids::DRAWER_SETTINGS],
    );
    adapter().onboard(&device(&ui), &env()).await.unwrap();
    ui.swallows_taps(Ids::APPBAR_DRAWER, 1);

    let identity = adapter().displayed_identity(&device(&ui)).await.unwrap();

    assert_eq!(identity.public_key, "npub1alice");
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
    // A missing marker means the client is not the build under
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

/// A retained profile's relay list is not drawn the instant the relays
/// route opens. Asking once and moving on read "not listed", opened the add
/// dialog, and the app answered that the relay was already there while the
/// dialog covered the list -- which a live recovery reported as the relay
/// row not appearing within the whole UI timeout.
#[tokio::test]
async fn a_relay_list_that_is_still_drawing_is_waited_for_not_re_added() {
    let ui = fresh_app();
    adapter().onboard(&device(&ui), &env()).await.unwrap();
    ui.on_tap_relabel(
        Ids::NODE_CUSTOM_CONFIRM,
        Ids::SETTINGS_MOSTRO_NODE_PUBKEY,
        PUBKEY,
    );
    adapter()
        .configure_node_and_relay(&device(&ui), &env())
        .await
        .unwrap();

    ui.appears_after(RELAY_ROW, 3);
    let before = ui.calls().len();
    adapter()
        .configure_node_and_relay(&device(&ui), &env())
        .await
        .unwrap();
    let new_calls = &ui.calls()[before..];
    assert!(
        !new_calls.contains(&"tap settings.relays.add".to_owned()),
        "{new_calls:?}"
    );
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
    // Choosing a currency closes the picker, like the real app.
    ui.on_tap_hide(
        "order.create.currency.USD",
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
    // My Trades loaded: empty until the order is listed.
    ui.on_tap_hide(Ids::ORDER_CONFIRM_HOME, &[Ids::TRADES_EMPTY]);
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
        &[
            Ids::PAY_INVOICE_TEXT,
            Ids::PAY_ORDER_ID,
            Ids::PAY_NWC,
            Ids::PAY_CANCEL,
        ],
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
        Ids::TRADES_EMPTY,
        Ids::NAV_ORDER_BOOK,
        Ids::ORDER_BOOK_TAB_BUY,
        Ids::ORDER_BOOK_TAB_SELL,
    ]);
    ui.label(Ids::ORDER_STATUS, "pending");
    ui.label(Ids::ORDER_ID, "o1");
    ui.label(Ids::PAY_INVOICE_TEXT, "lnbc1000n1secretinvoice");
    ui.label(Ids::PAY_ORDER_ID, "o1");
    ui.label(Ids::INVOICE_NWC_TEXT, "lnbc1000n1buyerinvoice");
    ui
}

fn draft() -> OrderDraft {
    OrderDraft::fixed(OrderKind::Sell, 1000, "USD", 1, "cash")
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

/// A create form that keeps submit disabled with every field filled gets
/// its payment method and price entered once more; if that does not
/// enable it, the product is asked nothing further and the order is never
/// submitted.
#[tokio::test]
async fn a_create_form_that_stays_disabled_is_filled_again_once_then_reported() {
    let ui = app_with_orders();
    adapter().onboard(&device(&ui), &env()).await.unwrap();
    ui.disables(&[Ids::ORDER_CREATE_SUBMIT]);
    ui.present(&[Ids::ORDER_CREATE_CANCEL]);

    let error = adapter()
        .create_order(&device(&ui), &draft())
        .await
        .unwrap_err();

    assert_eq!(error.category, mortsom_common::FailureCategory::Product);
    assert!(error.message.contains("kept submit disabled"), "{error}");
    let calls = ui.calls();
    assert_eq!(
        calls
            .iter()
            .filter(|c| c.starts_with("type order.create.payment_method"))
            .count(),
        2,
        "{calls:?}"
    );
    assert!(
        calls.contains(&"tap order.create.cancel".to_owned()),
        "{calls:?}"
    );
    assert!(
        !calls.contains(&"tap order.create.submit".to_owned()),
        "{calls:?}"
    );
}

/// The currency picker's lazy list rebuilds its rows while it filters, and
/// a press delivered to a row it has just replaced reaches nothing. Bob's
/// live Web buy of 2026-09-09 stopped with the picker still open over the
/// form. Choosing is idempotent, so a lost pick is pressed again; a submit
/// never is (see the swallowed-submit test).
#[tokio::test]
async fn a_currency_pick_the_picker_ignored_is_pressed_again_until_it_closes() {
    let ui = app_with_orders();
    adapter().onboard(&device(&ui), &env()).await.unwrap();
    ui.swallows_taps("order.create.currency.USD", 1);

    let order = adapter()
        .create_order(&device(&ui), &draft())
        .await
        .unwrap();

    assert_eq!(order.state, OrderState::Pending);
    let picks = ui
        .calls()
        .iter()
        .filter(|c| *c == "tap order.create.currency.USD")
        .count();
    assert_eq!(picks, 2, "{:?}", ui.calls());
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

/// A dispute is opened from the trade detail and confirmed in its dialog;
/// the app's dispute screen that follows is left for the next observation
/// to navigate away from.
#[tokio::test]
async fn a_dispute_is_opened_from_the_trade_detail_and_confirmed() {
    let ui = app_with_orders();
    // An active trade offers the dispute control beside its status.
    ui.on_tap(
        "trades.item.o1",
        &[
            Ids::ORDER_STATUS,
            Ids::ORDER_ID,
            Ids::APPBAR_BACK,
            Ids::TRADE_DISPUTE,
        ],
    );
    ui.on_tap(Ids::TRADE_DISPUTE, &[Ids::TRADE_DISPUTE_CONFIRM]);
    let a = adapter();
    a.onboard(&device(&ui), &env()).await.unwrap();
    a.create_order(&device(&ui), &draft()).await.unwrap();
    ui.label(Ids::ORDER_STATUS, "active");

    a.open_dispute(&device(&ui), &OrderReference::new("o1"))
        .await
        .unwrap();

    let calls = ui.calls();
    let dispute = calls
        .iter()
        .position(|c| c == "tap trade.dispute")
        .expect("the dispute control is tapped");
    let confirm = calls
        .iter()
        .position(|c| c == "tap trade.dispute.confirm")
        .expect("the dialog is confirmed");
    assert!(dispute < confirm);
}

/// An order the app offers no dispute control for is reported as a
/// contract gap, never confirmed blindly.
#[tokio::test]
async fn a_missing_dispute_control_is_a_contract_gap() {
    let ui = app_with_orders();
    let a = adapter();
    a.onboard(&device(&ui), &env()).await.unwrap();
    a.create_order(&device(&ui), &draft()).await.unwrap();
    ui.label(Ids::ORDER_STATUS, "pending");

    let error = a
        .open_dispute(&device(&ui), &OrderReference::new("o1"))
        .await
        .unwrap_err();

    assert_eq!(
        error.category,
        mortsom_common::FailureCategory::AutomationContract
    );
    assert!(!ui.calls().contains(&"tap trade.dispute.confirm".to_owned()));
}

#[tokio::test]
async fn cancellation_waits_for_the_trade_list_to_finish_navigation() {
    let ui = app_with_orders();
    let a = adapter();
    a.onboard(&device(&ui), &env()).await.unwrap();
    a.create_order(&device(&ui), &draft()).await.unwrap();
    ui.label(Ids::ORDER_STATUS, "pending");
    ui.appears_after("trades.item.o1", 2);

    assert_eq!(
        a.cancel_if_open(&device(&ui), &OrderReference::new("o1"))
            .await
            .unwrap(),
        CancelOutcome::Cancelled
    );
    assert!(ui.calls().contains(&"tap trade.cancel.confirm".to_owned()));
}

#[tokio::test]
async fn missing_cancel_control_does_not_prove_an_open_or_unknown_order_is_closed() {
    for status in ["active", "loading", "unknown-state"] {
        let ui = app_with_orders();
        let a = adapter();
        a.onboard(&device(&ui), &env()).await.unwrap();
        a.create_order(&device(&ui), &draft()).await.unwrap();
        ui.label(Ids::ORDER_STATUS, status);
        ui.on_tap_hide("trades.item.o1", &[Ids::TRADE_CANCEL]);

        let error = a
            .cancel_if_open(&device(&ui), &OrderReference::new("o1"))
            .await
            .unwrap_err();

        assert_eq!(
            error.category,
            mortsom_common::FailureCategory::AutomationContract
        );
        assert!(!ui.calls().contains(&"tap trade.cancel.confirm".to_owned()));
    }
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

#[tokio::test]
async fn an_enabled_submit_after_a_confirmation_timeout_is_never_republished() {
    let ui = app_with_orders();
    adapter().onboard(&device(&ui), &env()).await.unwrap();
    ui.appears_after(Ids::ORDER_CONFIRM_HOME, 100);
    let _outcome = adapter().create_order(&device(&ui), &draft()).await;
    assert_eq!(
        ui.calls()
            .iter()
            .filter(|call| *call == "tap order.create.submit")
            .count(),
        1
    );
}

#[tokio::test]
async fn recovery_refuses_an_empty_identity_without_generating_a_replacement() {
    let ui = fresh_app();
    adapter().onboard(&device(&ui), &env()).await.unwrap();
    ui.label(Ids::KEYS_PUBLIC_KEY, "");
    let error = adapter().existing_identity(&device(&ui)).await.unwrap_err();
    assert!(error.message.contains("identity is missing"));
    assert!(!ui.calls().iter().any(|call| call == "tap keys.generate"));
}

#[path = "tests_invoice.rs"]
mod invoice;
#[path = "tests_pricing.rs"]
mod pricing;
#[path = "tests_take.rs"]
mod take;
#[path = "tests_trades_list.rs"]
mod trades_list;
