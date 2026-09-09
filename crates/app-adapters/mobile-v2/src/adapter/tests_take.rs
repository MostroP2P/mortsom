//! Taking an order may land directly on its invoice form, before trade detail.
use super::*;

const PAY_ORDER_ID: &str = "pay.order_id";

fn taken_form(kind: OrderKind, identity: &str) -> FakeUi {
    let ui = FakeUi::default();
    ui.present(&[
        Ids::ORDER_ADD_FAB,
        Ids::ORDER_BOOK_TAB_BUY,
        Ids::ORDER_BOOK_TAB_SELL,
    ]);
    let (tab, form, id) = match kind {
        OrderKind::Sell => (
            Ids::ORDER_BOOK_TAB_BUY,
            Ids::INVOICE_TEXT,
            Ids::INVOICE_ORDER_ID,
        ),
        OrderKind::Buy => (
            Ids::ORDER_BOOK_TAB_SELL,
            Ids::PAY_INVOICE_TEXT,
            PAY_ORDER_ID,
        ),
    };
    ui.on_tap(tab, &["order.book.item.o1"]);
    ui.on_tap("order.book.item.o1", &[Ids::ORDER_TAKE_CONFIRM]);
    ui.on_tap(Ids::ORDER_TAKE_CONFIRM, &[form, id]);
    ui.on_tap_hide(
        Ids::ORDER_TAKE_CONFIRM,
        &[
            Ids::ORDER_ADD_FAB,
            Ids::ORDER_BOOK_TAB_BUY,
            Ids::ORDER_BOOK_TAB_SELL,
            Ids::ORDER_TAKE_CONFIRM,
        ],
    );
    ui.label(id, identity);
    ui.label(Ids::PAY_INVOICE_TEXT, "private-hold-invoice");
    ui
}

#[tokio::test]
async fn sell_take_accepts_only_the_bound_buyer_invoice_form_and_observes_taken() {
    take_and_observe_form(OrderKind::Sell, "waiting-invoice").await;
}

#[tokio::test]
async fn buy_take_accepts_only_the_bound_seller_pay_form_and_observes_taken() {
    take_and_observe_form(OrderKind::Buy, "waiting-payment").await;
}

async fn take_and_observe_form(kind: OrderKind, raw: &str) {
    let ui = taken_form(kind, "o1");
    let order = OrderReference::with_kind("o1", kind);
    let a = adapter();
    a.find_and_take_order(&device(&ui), &order).await.unwrap();
    let before = ui.calls();
    let state = a.observe_order(&device(&ui), &order).await.unwrap();
    assert_eq!(state.state, OrderState::Taken);
    assert_eq!(state.raw_label, raw);
    assert_eq!(
        ui.calls(),
        before,
        "reading the form must not navigate away"
    );
    assert_eq!(
        before
            .iter()
            .filter(|call| *call == "tap order.take.confirm")
            .count(),
        1
    );
}

#[tokio::test]
async fn a_foreign_take_form_never_proves_the_requested_order_was_taken() {
    for kind in [OrderKind::Sell, OrderKind::Buy] {
        let ui = taken_form(kind, "other-order");
        let order = OrderReference::with_kind("o1", kind);
        assert!(
            adapter()
                .find_and_take_order(&device(&ui), &order)
                .await
                .is_err()
        );
        assert!(adapter().observe_order(&device(&ui), &order).await.is_err());
    }
}

#[tokio::test]
async fn existing_bound_seller_form_exposes_the_invoice_without_reopening_or_paying() {
    let ui = FakeUi::default();
    ui.present(&[Ids::PAY_INVOICE_TEXT, PAY_ORDER_ID]);
    ui.label(PAY_ORDER_ID, "o1");
    ui.label(Ids::PAY_INVOICE_TEXT, "private-hold-invoice");
    let handoff = adapter()
        .pay_hold_invoice(
            &device(&ui),
            &OrderReference::with_kind("o1", OrderKind::Buy),
            PaymentPath::ExternalWallet,
        )
        .await
        .unwrap();
    assert_eq!(handoff.invoice.unwrap().expose(), "private-hold-invoice");
    assert!(
        ui.calls().is_empty(),
        "manual payment only reads the bound form"
    );
}

#[tokio::test]
async fn foreign_seller_form_never_exposes_an_invoice_for_payment() {
    let ui = FakeUi::default();
    ui.present(&[Ids::PAY_INVOICE_TEXT, PAY_ORDER_ID]);
    ui.label(PAY_ORDER_ID, "other-order");
    ui.label(Ids::PAY_INVOICE_TEXT, "private-hold-invoice");
    assert!(
        adapter()
            .pay_hold_invoice(
                &device(&ui),
                &OrderReference::with_kind("o1", OrderKind::Buy),
                PaymentPath::ExternalWallet,
            )
            .await
            .is_err()
    );
    assert!(ui.calls().is_empty());
}

#[tokio::test]
async fn buyer_submission_leaves_the_form_and_observes_the_returned_trade_status() {
    let ui = taken_form(OrderKind::Sell, "o1");
    let a = adapter();
    let order = OrderReference::with_kind("o1", OrderKind::Sell);
    a.find_and_take_order(&device(&ui), &order).await.unwrap();
    ui.present(&[Ids::INVOICE_SUBMIT]);
    ui.on_tap(
        Ids::INVOICE_SUBMIT,
        &[
            Ids::ORDER_STATUS,
            Ids::ORDER_ID,
            Ids::ORDER_ADD_FAB,
            Ids::NAV_TRADES,
            "trades.item.o1",
        ],
    );
    ui.on_tap_hide(
        Ids::INVOICE_SUBMIT,
        &[Ids::INVOICE_TEXT, Ids::INVOICE_ORDER_ID],
    );
    ui.label(Ids::ORDER_ID, "o1");
    ui.label(Ids::ORDER_STATUS, "active");
    a.provide_buyer_invoice(&device(&ui), &order, Some(&Secret::new("private-invoice")))
        .await
        .unwrap();
    let state = a.observe_order(&device(&ui), &order).await.unwrap();
    assert_eq!(state.state, OrderState::Active);
    assert_eq!(state.raw_label, "active");
}

/// A detail screen in transition reads its order id blank or stale for a
/// moment. The live Web run of 2026-09-09 polled Alice's screen while a
/// route settled and rejected her own trade as "a different order"; only a
/// readout that stays wrong after settling is foreign.
#[tokio::test]
async fn an_order_id_still_settling_is_waited_for_not_rejected() {
    let ui = taken_form(OrderKind::Sell, "o1");
    let a = adapter();
    let order = OrderReference::with_kind("o1", OrderKind::Sell);
    a.find_and_take_order(&device(&ui), &order).await.unwrap();
    ui.present(&[Ids::INVOICE_SUBMIT]);
    ui.on_tap(
        Ids::INVOICE_SUBMIT,
        &[
            Ids::ORDER_STATUS,
            Ids::ORDER_ID,
            Ids::ORDER_ADD_FAB,
            Ids::NAV_TRADES,
            "trades.item.o1",
        ],
    );
    ui.on_tap_hide(
        Ids::INVOICE_SUBMIT,
        &[Ids::INVOICE_TEXT, Ids::INVOICE_ORDER_ID],
    );
    ui.label(Ids::ORDER_ID, "o1");
    ui.label(Ids::ORDER_STATUS, "fiat-sent");
    a.provide_buyer_invoice(&device(&ui), &order, Some(&Secret::new("private-invoice")))
        .await
        .unwrap();

    ui.label_settles(Ids::ORDER_ID, "", 2);
    let state = a.observe_order(&device(&ui), &order).await.unwrap();
    assert_eq!(state.state, OrderState::FiatSent);

    ui.label(Ids::ORDER_ID, "o2");
    let failure = a.observe_order(&device(&ui), &order).await.unwrap_err();
    assert!(
        failure.message.contains("shows `o2`, expected `o1`"),
        "{failure:?}"
    );
}

/// The maker of a buy lands on the invoice form before the seller's hold is
/// accepted; the daemon's request then rebuilds the form and drops the typed
/// invoice. Bob's live Web buy of 2026-09-09 pressed a disabled submit over
/// an empty field. The invoice is typed again; submit is still pressed once.
#[tokio::test]
async fn an_invoice_the_app_dropped_is_typed_again_and_submitted_once() {
    let ui = taken_form(OrderKind::Sell, "o1");
    let a = adapter();
    let order = OrderReference::with_kind("o1", OrderKind::Sell);
    a.find_and_take_order(&device(&ui), &order).await.unwrap();
    ui.present(&[Ids::INVOICE_SUBMIT]);
    ui.on_tap(
        Ids::INVOICE_SUBMIT,
        &[
            Ids::ORDER_STATUS,
            Ids::ORDER_ID,
            Ids::ORDER_ADD_FAB,
            Ids::NAV_TRADES,
            "trades.item.o1",
        ],
    );
    ui.on_tap_hide(
        Ids::INVOICE_SUBMIT,
        &[Ids::INVOICE_TEXT, Ids::INVOICE_ORDER_ID],
    );
    ui.label(Ids::ORDER_ID, "o1");
    ui.label(Ids::ORDER_STATUS, "active");
    ui.wipes_text(Ids::INVOICE_TEXT, 1);

    a.provide_buyer_invoice(&device(&ui), &order, Some(&Secret::new("private-invoice")))
        .await
        .unwrap();

    let calls = ui.calls();
    assert_eq!(
        calls
            .iter()
            .filter(|c| c.starts_with("type invoice.text"))
            .count(),
        2,
        "{calls:?}"
    );
    assert_eq!(
        calls.iter().filter(|c| *c == "tap invoice.submit").count(),
        1,
        "{calls:?}"
    );
}

/// Opening a trade that waits on this user's invoice lands on the invoice
/// form, whose cancel button cancels the order. Cleanup must use it instead
/// of waiting for a detail screen that never comes.
#[tokio::test]
async fn cancel_from_the_invoice_form_uses_the_forms_cancel_button() {
    let ui = taken_form(OrderKind::Sell, "o1");
    let a = adapter();
    let order = OrderReference::with_kind("o1", OrderKind::Sell);
    a.find_and_take_order(&device(&ui), &order).await.unwrap();
    // Back leaves the form for the book, where the trade is listed; opening
    // it returns to the form.
    ui.present(&[Ids::NAV_TRADES, "trades.item.o1", Ids::APPBAR_BACK]);
    ui.on_tap(Ids::APPBAR_BACK, &[Ids::ORDER_ADD_FAB]);
    ui.on_tap_hide(
        Ids::APPBAR_BACK,
        &[Ids::INVOICE_TEXT, Ids::INVOICE_ORDER_ID],
    );
    ui.on_tap(
        "trades.item.o1",
        &[
            Ids::INVOICE_TEXT,
            Ids::INVOICE_ORDER_ID,
            Ids::INVOICE_CANCEL,
        ],
    );
    ui.on_tap(Ids::INVOICE_CANCEL, &[Ids::TRADE_CANCEL_CONFIRM]);

    let outcome = a.cancel_if_open(&device(&ui), &order).await.unwrap();

    assert_eq!(outcome, CancelOutcome::Cancelled);
    let calls = ui.calls();
    assert!(
        calls.contains(&"tap invoice.cancel".to_owned()),
        "{calls:?}"
    );
    assert!(
        calls.contains(&"tap trade.cancel.confirm".to_owned()),
        "{calls:?}"
    );
}

/// The detail read leaves an open form and reopens the trade: a trade the
/// app routes straight back to the invoice form is one the daemon has asked
/// an invoice for; one that opens on its detail shows that status instead.
#[tokio::test]
async fn the_detail_read_reports_where_the_reopened_trade_lands() {
    let ui = taken_form(OrderKind::Sell, "o1");
    let a = adapter();
    let order = OrderReference::with_kind("o1", OrderKind::Sell);
    a.find_and_take_order(&device(&ui), &order).await.unwrap();
    ui.present(&[Ids::NAV_TRADES, "trades.item.o1", Ids::APPBAR_BACK]);
    ui.on_tap(Ids::APPBAR_BACK, &[Ids::ORDER_ADD_FAB]);
    ui.on_tap_hide(
        Ids::APPBAR_BACK,
        &[Ids::INVOICE_TEXT, Ids::INVOICE_ORDER_ID],
    );
    ui.on_tap(
        "trades.item.o1",
        &[Ids::INVOICE_TEXT, Ids::INVOICE_ORDER_ID],
    );

    let state = a.observe_order_detail(&device(&ui), &order).await.unwrap();
    assert_eq!(state.raw_label, "waiting-invoice");
    assert_eq!(state.state, OrderState::Taken);

    // Before the daemon asks, the same trade opens on its detail.
    let ui = taken_form(OrderKind::Sell, "o1");
    a.find_and_take_order(&device(&ui), &order).await.unwrap();
    ui.present(&[Ids::NAV_TRADES, "trades.item.o1", Ids::APPBAR_BACK]);
    ui.on_tap(Ids::APPBAR_BACK, &[Ids::ORDER_ADD_FAB]);
    ui.on_tap_hide(
        Ids::APPBAR_BACK,
        &[Ids::INVOICE_TEXT, Ids::INVOICE_ORDER_ID],
    );
    ui.on_tap("trades.item.o1", &[Ids::ORDER_STATUS, Ids::ORDER_ID]);
    ui.label(Ids::ORDER_ID, "o1");
    ui.label(Ids::ORDER_STATUS, "waiting-payment");
    let state = a.observe_order_detail(&device(&ui), &order).await.unwrap();
    assert_eq!(state.raw_label, "waiting-payment");
}
