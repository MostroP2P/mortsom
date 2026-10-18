//! The anti-abuse bond journeys: a take that lands on the pay-bond screen
//! and reads `waiting-bond`, the bond bolt11 handed over for the caller's
//! node to pay (or paid by the connected wallet), the walk back from the
//! bond, and the parked trade's way back to its bond screen.
use super::*;

const BOND_INVOICE: &str = "lnbcrt10u1secretbondinvoice";

/// The book, a sell order in it, and a take that a bonded node answers
/// with the pay-bond screen instead of the buyer invoice form.
fn take_on_a_bonded_node() -> FakeUi {
    let ui = FakeUi::default();
    ui.present(&[
        Ids::ORDER_ADD_FAB,
        Ids::ORDER_BOOK_TAB_BUY,
        Ids::ORDER_BOOK_TAB_SELL,
    ]);
    ui.on_tap(Ids::ORDER_BOOK_TAB_BUY, &["order.book.item.o1"]);
    ui.on_tap("order.book.item.o1", &[Ids::ORDER_TAKE_CONFIRM]);
    ui.on_tap(
        Ids::ORDER_TAKE_CONFIRM,
        &[Ids::BOND_INVOICE_TEXT, Ids::BOND_ORDER_ID, Ids::BOND_CANCEL],
    );
    ui.on_tap_hide(
        Ids::ORDER_TAKE_CONFIRM,
        &[
            Ids::ORDER_ADD_FAB,
            Ids::ORDER_BOOK_TAB_BUY,
            Ids::ORDER_BOOK_TAB_SELL,
            Ids::ORDER_TAKE_CONFIRM,
        ],
    );
    ui.label(Ids::BOND_ORDER_ID, "o1");
    ui.label(Ids::BOND_INVOICE_TEXT, BOND_INVOICE);
    // Walking away asks first, then returns to the book.
    ui.on_tap(Ids::BOND_CANCEL, &[Ids::BOND_CANCEL_CONFIRM]);
    ui.on_tap(Ids::BOND_CANCEL_CONFIRM, &[Ids::ORDER_ADD_FAB]);
    ui.on_tap_hide(
        Ids::BOND_CANCEL_CONFIRM,
        &[
            Ids::BOND_INVOICE_TEXT,
            Ids::BOND_ORDER_ID,
            Ids::BOND_CANCEL,
            Ids::BOND_CANCEL_CONFIRM,
        ],
    );
    ui
}

fn taker(ui: &FakeUi) -> DeviceContext<'_> {
    DeviceContext {
        actor: Actor::Bob,
        ui,
        ui_timeout: Duration::from_millis(200),
    }
}

#[tokio::test]
async fn a_take_on_a_bonded_node_lands_on_the_pay_bond_screen_and_reads_waiting_bond() {
    let ui = take_on_a_bonded_node();
    let order = OrderReference::with_kind("o1", OrderKind::Sell);
    let a = adapter();

    a.find_and_take_order(&taker(&ui), &order).await.unwrap();
    let state = a.observe_order(&taker(&ui), &order).await.unwrap();

    assert_eq!(state.state, OrderState::Taken);
    assert_eq!(state.raw_label, "waiting-bond");
    // The take never pressed anything on the bond screen.
    let calls = ui.calls().join("\n");
    assert!(!calls.contains("tap bond."), "{calls}");
    assert!(!calls.contains("tap pay.nwc"), "{calls}");
}

#[tokio::test]
async fn the_bond_invoice_is_handed_over_without_the_app_paying_it() {
    let ui = take_on_a_bonded_node();
    let order = OrderReference::with_kind("o1", OrderKind::Sell);
    let a = adapter();
    a.find_and_take_order(&taker(&ui), &order).await.unwrap();

    let handoff = a
        .pay_bond_invoice(&taker(&ui), &order, PaymentPath::ExternalWallet)
        .await
        .unwrap();

    assert_eq!(handoff.kind, PaymentPath::ExternalWallet);
    assert_eq!(
        handoff.invoice.as_ref().map(|invoice| invoice.expose()),
        Some(BOND_INVOICE)
    );
    let calls = ui.calls().join("\n");
    assert!(!calls.contains("tap pay.nwc"), "{calls}");
    assert!(!calls.contains("secretbond"), "{calls}");
}

/// With a wallet connected the bond screen draws the escrow's pay widget:
/// `pay.invoice.text` and `pay.nwc` under the bond's own app bar, and no
/// `bond.invoice.text` at all.
fn take_on_a_bonded_node_with_a_wallet() -> FakeUi {
    let ui = FakeUi::default();
    ui.present(&[
        Ids::ORDER_ADD_FAB,
        Ids::ORDER_BOOK_TAB_BUY,
        Ids::ORDER_BOOK_TAB_SELL,
    ]);
    ui.on_tap(Ids::ORDER_BOOK_TAB_BUY, &["order.book.item.o1"]);
    ui.on_tap("order.book.item.o1", &[Ids::ORDER_TAKE_CONFIRM]);
    ui.on_tap(
        Ids::ORDER_TAKE_CONFIRM,
        &[Ids::BOND_ORDER_ID, Ids::PAY_INVOICE_TEXT, Ids::PAY_NWC],
    );
    ui.on_tap_hide(
        Ids::ORDER_TAKE_CONFIRM,
        &[
            Ids::ORDER_ADD_FAB,
            Ids::ORDER_BOOK_TAB_BUY,
            Ids::ORDER_BOOK_TAB_SELL,
            Ids::ORDER_TAKE_CONFIRM,
        ],
    );
    ui.label(Ids::BOND_ORDER_ID, "o1");
    ui.label(Ids::PAY_INVOICE_TEXT, BOND_INVOICE);
    // The bond locks: the buyer's next step is the invoice form.
    ui.on_tap(Ids::PAY_NWC, &[Ids::INVOICE_TEXT, Ids::INVOICE_ORDER_ID]);
    ui.on_tap_hide(
        Ids::PAY_NWC,
        &[Ids::BOND_ORDER_ID, Ids::PAY_INVOICE_TEXT, Ids::PAY_NWC],
    );
    ui
}

#[tokio::test]
async fn with_a_wallet_connected_the_bond_screen_is_still_read_as_waiting_bond() {
    let ui = take_on_a_bonded_node_with_a_wallet();
    let order = OrderReference::with_kind("o1", OrderKind::Sell);
    let a = adapter();

    a.find_and_take_order(&taker(&ui), &order).await.unwrap();
    let state = a.observe_order(&taker(&ui), &order).await.unwrap();

    // Not the escrow's `waiting-payment`: the app bar says it is the bond.
    assert_eq!(state.raw_label, "waiting-bond");
}

#[tokio::test]
async fn with_a_wallet_connected_the_app_pays_the_bond_and_moves_to_the_trade() {
    let ui = take_on_a_bonded_node_with_a_wallet();
    let order = OrderReference::with_kind("o1", OrderKind::Sell);
    let a = adapter();
    a.find_and_take_order(&taker(&ui), &order).await.unwrap();

    let handoff = a
        .pay_bond_invoice(&taker(&ui), &order, PaymentPath::Nwc)
        .await
        .unwrap();

    assert_eq!(handoff.kind, PaymentPath::Nwc);
    assert_eq!(
        handoff.invoice.as_ref().map(|invoice| invoice.expose()),
        Some(BOND_INVOICE),
        "the hash is still correlated"
    );
    assert!(ui.calls().contains(&"tap pay.nwc".to_owned()));
}

#[tokio::test]
async fn a_bond_screen_of_another_order_cannot_authorize_a_payment() {
    let ui = take_on_a_bonded_node();
    let order = OrderReference::with_kind("o1", OrderKind::Sell);
    let a = adapter();
    a.find_and_take_order(&taker(&ui), &order).await.unwrap();
    ui.label(Ids::BOND_ORDER_ID, "o2");

    let err = a
        .pay_bond_invoice(&taker(&ui), &order, PaymentPath::ExternalWallet)
        .await
        .unwrap_err();

    assert_eq!(
        err.category,
        mortsom_common::FailureCategory::AutomationContract
    );
    assert!(err.message.contains("different order"), "{err}");
}

#[tokio::test]
async fn an_empty_bond_screen_is_a_product_failure_not_an_empty_handoff() {
    let ui = take_on_a_bonded_node();
    let order = OrderReference::with_kind("o1", OrderKind::Sell);
    let a = adapter();
    a.find_and_take_order(&taker(&ui), &order).await.unwrap();
    ui.label(Ids::BOND_INVOICE_TEXT, "   ");

    let err = a
        .pay_bond_invoice(&taker(&ui), &order, PaymentPath::ExternalWallet)
        .await
        .unwrap_err();

    assert_eq!(err.category, mortsom_common::FailureCategory::Product);
    assert!(err.message.contains("no bond to pay"), "{err}");
}

#[tokio::test]
async fn declining_the_bond_confirms_the_cancel_and_returns_to_the_book() {
    let ui = take_on_a_bonded_node();
    let order = OrderReference::with_kind("o1", OrderKind::Sell);
    let a = adapter();
    a.find_and_take_order(&taker(&ui), &order).await.unwrap();

    a.decline_bond(&taker(&ui), &order).await.unwrap();

    let calls = ui.calls();
    let cancel = calls.iter().position(|c| c == "tap bond.cancel").unwrap();
    let confirm = calls
        .iter()
        .position(|c| c == "tap bond.cancel.confirm")
        .unwrap();
    assert!(cancel < confirm, "{calls:?}");
    assert!(
        !calls.iter().any(|call| call.contains("trade.cancel")),
        "{calls:?}"
    );
    assert!(ui.has(Ids::ORDER_ADD_FAB));
}

/// A node that refuses the maker's cancel leaves the app offering to drop
/// the order from this device alone: that drops nothing on the daemon, so
/// it is reported as the product's answer and never pressed.
#[tokio::test]
async fn a_refused_cancel_on_the_bond_screen_is_a_product_failure() {
    let ui = FakeUi::default();
    ui.present(&[Ids::BOND_ORDER_ID, Ids::BOND_CANCEL]);
    ui.label(Ids::BOND_ORDER_ID, "o1");
    ui.on_tap(Ids::BOND_CANCEL, &[Ids::BOND_CANCEL_CONFIRM]);
    ui.on_tap(Ids::BOND_CANCEL_CONFIRM, &[Ids::BOND_REMOVE_FROM_DEVICE]);
    let order = OrderReference::with_kind("o1", OrderKind::Sell);

    let err = adapter()
        .decline_bond(&maker(&ui), &order)
        .await
        .unwrap_err();

    assert_eq!(err.category, mortsom_common::FailureCategory::Product);
    assert!(err.message.contains("refused"), "{err}");
    assert!(
        !ui.calls()
            .contains(&"tap bond.remove_from_device".to_owned()),
        "nothing is dropped from the device"
    );
}

/// A trade parked on its bond, reopened later: the trade detail reads
/// `waiting-bond` and offers the deposit action, which is the way back to
/// the bond screen.
fn parked_trade() -> FakeUi {
    let ui = FakeUi::default();
    ui.present(&[
        Ids::ORDER_ADD_FAB,
        Ids::NAV_TRADES,
        Ids::NAV_ORDER_BOOK,
        "trades.item.o1",
    ]);
    ui.on_tap(
        "trades.item.o1",
        &[
            Ids::ORDER_STATUS,
            Ids::ORDER_ID,
            Ids::TRADE_PAY_BOND,
            Ids::TRADE_CANCEL,
            Ids::APPBAR_BACK,
        ],
    );
    ui.on_tap(
        Ids::TRADE_PAY_BOND,
        &[Ids::BOND_INVOICE_TEXT, Ids::BOND_ORDER_ID, Ids::BOND_CANCEL],
    );
    ui.label(Ids::ORDER_STATUS, "waiting-bond");
    ui.label(Ids::ORDER_ID, "o1");
    ui.label(Ids::BOND_ORDER_ID, "o1");
    ui.label(Ids::BOND_INVOICE_TEXT, BOND_INVOICE);
    ui
}

#[tokio::test]
async fn a_parked_trade_reads_waiting_bond_from_its_detail() {
    let ui = parked_trade();
    let order = OrderReference::with_kind("o1", OrderKind::Sell);
    let a = adapter();

    let state = a.observe_order_detail(&taker(&ui), &order).await.unwrap();

    assert_eq!(state.state, OrderState::Taken);
    assert_eq!(state.raw_label, "waiting-bond");
}

#[tokio::test]
async fn a_parked_trade_reopens_its_bond_screen_from_the_deposit_action() {
    let ui = parked_trade();
    let order = OrderReference::with_kind("o1", OrderKind::Sell);
    let a = adapter();

    let handoff = a
        .pay_bond_invoice(&taker(&ui), &order, PaymentPath::ExternalWallet)
        .await
        .unwrap();

    assert_eq!(
        handoff.invoice.as_ref().map(|invoice| invoice.expose()),
        Some(BOND_INVOICE)
    );
    assert!(ui.calls().contains(&"tap trade.payBond".to_owned()));
}

/// A maker's own order parked on its deposit: `/my_order` reads
/// `waiting-bond` and offers `order.payBond`; the pay-bond screen opens
/// with the explainer unfolded, which hides the bolt11 until folded.
fn makers_order_on_its_bond() -> FakeUi {
    let ui = FakeUi::default();
    ui.present(&[
        Ids::ORDER_ADD_FAB,
        Ids::NAV_TRADES,
        Ids::NAV_ORDER_BOOK,
        "trades.item.o1",
    ]);
    ui.on_tap(
        "trades.item.o1",
        &[
            Ids::ORDER_STATUS,
            Ids::ORDER_ID,
            Ids::MY_ORDER_PAY_BOND,
            Ids::ORDER_CONFIRM_HOME,
        ],
    );
    ui.on_tap(
        Ids::MY_ORDER_PAY_BOND,
        &[Ids::BOND_ORDER_ID, Ids::BOND_EXPLAINER, Ids::BOND_CANCEL],
    );
    ui.on_tap(Ids::BOND_EXPLAINER, &[Ids::BOND_INVOICE_TEXT]);
    ui.label(Ids::ORDER_STATUS, "waiting-bond");
    ui.label(Ids::ORDER_ID, "o1");
    ui.label(Ids::BOND_ORDER_ID, "o1");
    ui.label(Ids::BOND_INVOICE_TEXT, BOND_INVOICE);
    ui
}

fn maker(ui: &FakeUi) -> DeviceContext<'_> {
    DeviceContext {
        actor: Actor::Alice,
        ui,
        ui_timeout: Duration::from_millis(200),
    }
}

#[tokio::test]
async fn a_makers_deposit_is_reached_from_its_own_order_and_the_explainer_is_folded_to_read_it() {
    let ui = makers_order_on_its_bond();
    let order = OrderReference::with_kind("o1", OrderKind::Sell);
    let a = adapter();

    let handoff = a
        .pay_bond_invoice(&maker(&ui), &order, PaymentPath::ExternalWallet)
        .await
        .unwrap();

    assert_eq!(
        handoff.invoice.as_ref().map(|invoice| invoice.expose()),
        Some(BOND_INVOICE)
    );
    let calls = ui.calls();
    assert!(calls.contains(&"tap order.payBond".to_owned()), "{calls:?}");
    assert!(
        calls.contains(&"tap bond.explainer".to_owned()),
        "{calls:?}"
    );
}

/// The winner's claim, from the trade detail's banner: the share and the
/// phase are read, and an invoice typed into the form is sent.
fn trade_with_a_claim(phase: &str) -> FakeUi {
    let ui = FakeUi::default();
    ui.present(&[
        Ids::ORDER_ADD_FAB,
        Ids::NAV_TRADES,
        Ids::NAV_ORDER_BOOK,
        "trades.item.o1",
    ]);
    ui.on_tap(
        "trades.item.o1",
        &[
            Ids::ORDER_STATUS,
            Ids::ORDER_ID,
            Ids::TRADE_BOND_CLAIM,
            Ids::TRADE_BOND_CLAIM_OPEN,
        ],
    );
    ui.on_tap(
        Ids::TRADE_BOND_CLAIM_OPEN,
        &[
            Ids::BOND_CLAIM_ORDER_ID,
            Ids::BOND_CLAIM_AMOUNT,
            Ids::BOND_CLAIM_STATUS,
            Ids::BOND_CLAIM_MANUAL,
        ],
    );
    ui.on_tap(
        Ids::BOND_CLAIM_MANUAL,
        &[Ids::BOND_CLAIM_TEXT, Ids::BOND_CLAIM_SUBMIT],
    );
    ui.on_tap_relabel(Ids::BOND_CLAIM_SUBMIT, Ids::BOND_CLAIM_STATUS, "submitted");
    ui.label(Ids::ORDER_STATUS, "cancelled");
    ui.label(Ids::ORDER_ID, "o1");
    ui.label(Ids::BOND_CLAIM_ORDER_ID, "o1");
    ui.label(Ids::BOND_CLAIM_AMOUNT, "500 sats");
    ui.label(Ids::BOND_CLAIM_STATUS, phase);
    ui.label(Ids::TRADE_BOND_CLAIM, phase);
    ui
}

#[tokio::test]
async fn a_pending_claim_is_read_from_the_trade_and_an_invoice_is_sent_through_the_manual_form() {
    let ui = trade_with_a_claim("pending");
    let order = OrderReference::with_kind("o1", OrderKind::Sell);
    let a = adapter();

    let claim = a.bond_claim(&maker(&ui), &order).await.unwrap().unwrap();
    assert_eq!(claim.amount_sats, 500);
    assert_eq!(claim.status, "pending");

    let phase = a
        .submit_bond_claim_invoice(&maker(&ui), &order, &Secret::new("lnbcrt5u1claim"))
        .await
        .unwrap();

    assert_eq!(phase, "submitted");
    let calls = ui.calls();
    assert!(
        calls.contains(&"tap bond.claim.manual".to_owned()),
        "{calls:?}"
    );
    assert!(
        calls.contains(&"tap bond.claim.submit".to_owned()),
        "{calls:?}"
    );
    assert!(!calls.join("\n").contains("lnbcrt5u1claim"), "{calls:?}");
}

#[tokio::test]
async fn a_trade_without_a_claim_reads_none() {
    let ui = parked_trade();
    let order = OrderReference::with_kind("o1", OrderKind::Sell);

    let claim = adapter().bond_claim(&taker(&ui), &order).await.unwrap();

    assert_eq!(claim, None);
}

/// A timeout slash wipes the loser's trade row: the claim is all that is
/// left, and its row opens the claim screen directly.
#[tokio::test]
async fn a_claim_without_a_trade_opens_its_own_screen() {
    let ui = FakeUi::default();
    ui.present(&[
        Ids::ORDER_ADD_FAB,
        Ids::NAV_TRADES,
        Ids::NAV_ORDER_BOOK,
        "trades.item.o1",
    ]);
    ui.on_tap(
        "trades.item.o1",
        &[
            Ids::BOND_CLAIM_ORDER_ID,
            Ids::BOND_CLAIM_AMOUNT,
            Ids::BOND_CLAIM_STATUS,
        ],
    );
    ui.label(Ids::BOND_CLAIM_ORDER_ID, "o1");
    ui.label(Ids::BOND_CLAIM_AMOUNT, "500");
    ui.label(Ids::BOND_CLAIM_STATUS, "completed");
    let order = OrderReference::with_kind("o1", OrderKind::Sell);

    let claim = adapter()
        .bond_claim(&maker(&ui), &order)
        .await
        .unwrap()
        .unwrap();

    assert_eq!(claim.status, "completed");
    assert_eq!(claim.amount_sats, 500);
}

#[tokio::test]
async fn a_claim_screen_of_another_order_is_refused() {
    let ui = trade_with_a_claim("pending");
    ui.label(Ids::BOND_CLAIM_ORDER_ID, "o2");
    let order = OrderReference::with_kind("o1", OrderKind::Sell);

    let err = adapter().bond_claim(&maker(&ui), &order).await.unwrap_err();

    assert_eq!(
        err.category,
        mortsom_common::FailureCategory::AutomationContract
    );
}

#[tokio::test]
async fn the_slash_notice_is_read_from_the_losers_trade_and_is_none_once_the_row_is_gone() {
    let ui = parked_trade();
    ui.present(&[Ids::TRADE_BOND_SLASHED]);
    ui.label(Ids::TRADE_BOND_SLASHED, "dispute");
    let order = OrderReference::with_kind("o1", OrderKind::Sell);
    let a = adapter();

    let cause = a.bond_slash_notice(&taker(&ui), &order).await.unwrap();
    assert_eq!(cause.as_deref(), Some("dispute"));

    let gone = FakeUi::default();
    gone.present(&[Ids::ORDER_ADD_FAB, Ids::NAV_TRADES, Ids::NAV_ORDER_BOOK]);
    let none = a
        .bond_slash_notice(
            &taker(&gone),
            &OrderReference::with_kind("o1", OrderKind::Sell),
        )
        .await
        .unwrap();
    assert_eq!(none, None);
}

/// The cleanup sweep meets a maker's order parked on its deposit: it is
/// cancelled on its bond screen, where the app sends the daemon the cancel
/// once confirmed and closes to the book.
#[tokio::test]
async fn a_parked_makers_order_is_cleaned_up_on_its_bond_screen() {
    let ui = makers_order_on_its_bond();
    ui.on_tap(Ids::BOND_CANCEL, &[Ids::BOND_CANCEL_CONFIRM]);
    ui.on_tap(Ids::BOND_CANCEL_CONFIRM, &[Ids::ORDER_ADD_FAB]);
    let order = OrderReference::with_kind("o1", OrderKind::Sell);

    let outcome = adapter().cancel_if_open(&maker(&ui), &order).await.unwrap();

    assert_eq!(outcome, CancelOutcome::Cancelled);
    let calls = ui.calls();
    assert!(calls.contains(&"tap order.payBond".to_owned()), "{calls:?}");
    assert!(calls.contains(&"tap bond.cancel".to_owned()), "{calls:?}");
    assert!(
        calls.contains(&"tap bond.cancel.confirm".to_owned()),
        "{calls:?}"
    );
    assert!(
        !calls.iter().any(|c| c.contains("trade.cancel")),
        "{calls:?}"
    );
}

/// A row the app keeps for a slashed bond's claim alone opens the claim:
/// the trade behind it ended without completing, as the row itself says.
#[tokio::test]
async fn a_claim_only_row_reads_as_a_closed_order() {
    let ui = FakeUi::default();
    ui.present(&[
        Ids::ORDER_ADD_FAB,
        Ids::NAV_TRADES,
        Ids::NAV_ORDER_BOOK,
        "trades.item.o1",
    ]);
    ui.on_tap(
        "trades.item.o1",
        &[Ids::BOND_CLAIM_STATUS, Ids::BOND_CLAIM_ORDER_ID],
    );
    ui.label(Ids::BOND_CLAIM_STATUS, "pending");
    ui.label(Ids::BOND_CLAIM_ORDER_ID, "o1");
    let order = OrderReference::with_kind("o1", OrderKind::Sell);

    let shown = adapter().observe_order(&maker(&ui), &order).await.unwrap();

    assert_eq!(shown.state, OrderState::Cancelled);
    assert_eq!(shown.raw_label, "bond-claim");
}

/// The status readout is a machine name, but the sweep reads it the way
/// every other label is read: a build showing the status's display name
/// still has its parked order cancelled on the bond screen.
#[tokio::test]
async fn a_parked_order_is_recognised_whatever_case_its_status_reads_in() {
    let ui = makers_order_on_its_bond();
    ui.label(Ids::ORDER_STATUS, "Waiting Bond");
    ui.on_tap(Ids::BOND_CANCEL, &[Ids::BOND_CANCEL_CONFIRM]);
    ui.on_tap(Ids::BOND_CANCEL_CONFIRM, &[Ids::ORDER_ADD_FAB]);
    let order = OrderReference::with_kind("o1", OrderKind::Sell);

    let outcome = adapter().cancel_if_open(&maker(&ui), &order).await.unwrap();

    assert_eq!(outcome, CancelOutcome::Cancelled);
    assert!(ui.calls().contains(&"tap bond.cancel.confirm".to_owned()));
}
