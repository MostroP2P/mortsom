//! Back to the book from wherever the app is -- including a screen still
//! drawing, as after a look that was cut off mid-navigation, and a screen
//! leaving on its own between the look and the press.
use super::*;

#[tokio::test]
async fn going_home_waits_for_a_screen_still_drawing() {
    let ui = FakeUi::default();
    // A route mid-transition shows nothing to go by yet; its navigation
    // draws a few looks later.
    ui.present(&[Ids::NAV_ORDER_BOOK]);
    ui.appears_after(Ids::NAV_ORDER_BOOK, 3);
    ui.on_tap(Ids::NAV_ORDER_BOOK, &[Ids::ORDER_ADD_FAB]);

    nav::go_home(&device(&ui)).await.unwrap();

    assert!(ui.calls().contains(&"tap nav.order_book".to_owned()));
}

#[tokio::test]
async fn a_back_that_leaves_before_the_press_is_not_waited_for() {
    let ui = FakeUi::default();
    // The screen pops by itself: its back answers one look and goes, and
    // the book it returns to draws.
    ui.present(&[Ids::APPBAR_BACK, Ids::ORDER_ADD_FAB]);
    ui.vanishes_after(Ids::APPBAR_BACK, 1);
    ui.appears_after(Ids::ORDER_ADD_FAB, 1);

    nav::go_home(&device(&ui)).await.unwrap();
}

#[tokio::test]
async fn this_sides_standing_cancel_request_is_a_cancel_already_made() {
    let ui = FakeUi::default();
    let order = OrderReference::with_kind("o1", OrderKind::Sell);
    // A funded trade this side asked to cancel: the request stands, so the
    // detail offers no cancel of its own -- only the counterpart's answer
    // ends it.
    ui.present(&[Ids::ORDER_ADD_FAB, Ids::NAV_TRADES]);
    ui.on_tap(Ids::NAV_TRADES, &["trades.item.o1"]);
    ui.on_tap(
        "trades.item.o1",
        &[
            Ids::ORDER_STATUS,
            Ids::TRADE_CANCEL_REQUEST,
            Ids::TRADE_DISPUTE,
        ],
    );
    ui.label(Ids::ORDER_STATUS, "active");

    let outcome = adapter()
        .cancel_if_open(&device(&ui), &order)
        .await
        .unwrap();

    assert_eq!(outcome, CancelOutcome::Cancelled);
    assert!(!ui.calls().iter().any(|c| c.starts_with("tap trade.cancel")));
}

/// A trade that waits on this user's payment opens on the pay form, not the
/// detail: the form is the pay action, and its cancel the way out.
#[tokio::test]
async fn a_trade_opening_on_the_pay_form_offers_paying_and_cancelling() {
    let ui = FakeUi::default();
    let order = OrderReference::with_kind("o1", OrderKind::Sell);
    ui.present(&[Ids::ORDER_ADD_FAB, Ids::NAV_TRADES]);
    ui.on_tap(Ids::NAV_TRADES, &["trades.item.o1"]);
    ui.on_tap(
        "trades.item.o1",
        &[Ids::PAY_INVOICE_TEXT, Ids::PAY_ORDER_ID, Ids::PAY_CANCEL],
    );
    ui.label(Ids::PAY_ORDER_ID, "o1");

    let actions = adapter()
        .visible_actions(&device(&ui), &order)
        .await
        .unwrap();

    assert_eq!(actions, ["pay-invoice", "cancel"]);
}

/// Likewise a buyer owing the payout invoice lands on the invoice form.
#[tokio::test]
async fn a_trade_opening_on_the_invoice_form_offers_adding_it_and_cancelling() {
    let ui = FakeUi::default();
    let order = OrderReference::with_kind("o1", OrderKind::Sell);
    ui.present(&[Ids::ORDER_ADD_FAB, Ids::NAV_TRADES]);
    ui.on_tap(Ids::NAV_TRADES, &["trades.item.o1"]);
    ui.on_tap(
        "trades.item.o1",
        &[
            Ids::INVOICE_TEXT,
            Ids::INVOICE_ORDER_ID,
            Ids::INVOICE_CANCEL,
        ],
    );
    ui.label(Ids::INVOICE_ORDER_ID, "o1");

    let actions = adapter()
        .visible_actions(&device(&ui), &order)
        .await
        .unwrap();

    assert_eq!(actions, ["add-invoice", "cancel"]);
}
