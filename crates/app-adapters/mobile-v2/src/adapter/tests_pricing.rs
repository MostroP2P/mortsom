//! Orders beyond the fixed price for one amount: market pricing, ranges
//! taken at an amount, and the rating that follows a completed trade.
use super::*;
use mortsom_app_contract::OrderKind;

/// The order form as the real app draws it: the range segment swaps the one
/// amount field for a min/max pair, and the fixed-price segment reveals the
/// sats field. Taking the range order asks its amount right after the take
/// button; the completed trade offers the rate action.
fn app_with_pricing() -> FakeUi {
    let ui = app_with_orders();
    ui.on_tap(
        Ids::ORDER_ADD_SELL,
        &[
            Ids::ORDER_CREATE_CURRENCY,
            Ids::ORDER_CREATE_FIAT_AMOUNT,
            Ids::ORDER_CREATE_PAYMENT_METHOD_ADD,
            Ids::ORDER_CREATE_PRICE_TYPE,
            Ids::ORDER_CREATE_PRICE_MARKET,
            Ids::ORDER_CREATE_PRICE_FIXED,
            Ids::ORDER_CREATE_SUBMIT,
            Ids::ORDER_CREATE_RANGE,
            Ids::ORDER_CREATE_AMOUNT_RANGE,
        ],
    );
    ui.on_tap(
        Ids::ORDER_CREATE_AMOUNT_RANGE,
        &[Ids::ORDER_CREATE_FIAT_MIN, Ids::ORDER_CREATE_FIAT_MAX],
    );
    ui.on_tap_hide(
        Ids::ORDER_CREATE_AMOUNT_RANGE,
        &[Ids::ORDER_CREATE_FIAT_AMOUNT],
    );
    ui.on_tap(
        Ids::ORDER_TAKE_CONFIRM,
        &[Ids::ORDER_TAKE_AMOUNT, Ids::ORDER_TAKE_AMOUNT_CONFIRM],
    );
    ui.on_tap(
        Ids::ORDER_TAKE_AMOUNT_CONFIRM,
        &[Ids::ORDER_STATUS, Ids::ORDER_ID],
    );
    // The completed trade carries the rating itself: its stars, submit and
    // close, with no control that opens a rating screen first.
    ui.on_tap(
        "trades.item.o1",
        &[
            Ids::ORDER_STATUS,
            Ids::ORDER_ID,
            Ids::APPBAR_BACK,
            "trade.rate.star.5",
            Ids::TRADE_RATE_SUBMIT,
            Ids::TRADE_RATE_CLOSE,
        ],
    );
    ui.on_tap_relabel(Ids::TRADE_RATE_SUBMIT, Ids::ORDER_STATUS, "rated");
    ui
}

async fn form_calls(draft: &OrderDraft) -> Vec<String> {
    let ui = app_with_pricing();
    adapter().onboard(&device(&ui), &env()).await.unwrap();
    let order = adapter().create_order(&device(&ui), draft).await.unwrap();
    assert_eq!(order.id.as_deref(), Some("o1"));
    ui.calls()
}

/// A range order is the range segment plus both bounds, and it never
/// touches the price: the app locks a range to market pricing.
#[tokio::test]
async fn a_range_order_enters_both_bounds_and_leaves_the_price_on_market() {
    let calls = form_calls(&OrderDraft::range(OrderKind::Sell, "USD", 1, 3, "cash")).await;

    assert!(
        calls.contains(&"tap order.create.amount.range".to_owned()),
        "{calls:?}"
    );
    assert!(!calls.contains(&"tap order.create.range".to_owned()));
    assert!(calls.contains(&"type order.create.fiat_min (1 chars)".to_owned()));
    assert!(calls.contains(&"type order.create.fiat_max (1 chars)".to_owned()));
    assert!(!calls.iter().any(|c| c.contains("order.create.fiat_amount")));
    assert!(
        !calls
            .iter()
            .any(|c| c.starts_with("tap order.create.price"))
    );
    assert!(!calls.iter().any(|c| c.contains("order.create.sats_amount")));
}

/// The form starts on market price, so a market order types no sats and
/// leaves the price control alone.
#[tokio::test]
async fn a_market_order_types_no_sats_and_keeps_the_default_price_type() {
    let calls = form_calls(&OrderDraft::market(OrderKind::Sell, "USD", 1, "cash")).await;

    assert!(calls.contains(&"type order.create.fiat_amount (1 chars)".to_owned()));
    assert!(
        !calls
            .iter()
            .any(|c| c.starts_with("tap order.create.price"))
    );
    assert!(!calls.iter().any(|c| c.contains("order.create.sats_amount")));
}

/// A form already switched to fixed is switched back for a market order,
/// otherwise the sats field it kept would be submitted empty.
#[tokio::test]
async fn a_form_left_on_fixed_price_is_switched_back_to_market() {
    let ui = app_with_pricing();
    adapter().onboard(&device(&ui), &env()).await.unwrap();
    ui.on_tap(
        Ids::ORDER_ADD_SELL,
        &[
            Ids::ORDER_CREATE_CURRENCY,
            Ids::ORDER_CREATE_FIAT_AMOUNT,
            Ids::ORDER_CREATE_PAYMENT_METHOD_ADD,
            Ids::ORDER_CREATE_PRICE_TYPE,
            Ids::ORDER_CREATE_PRICE_MARKET,
            Ids::ORDER_CREATE_PRICE_FIXED,
            Ids::ORDER_CREATE_SATS_AMOUNT,
            Ids::ORDER_CREATE_SUBMIT,
        ],
    );

    adapter()
        .create_order(
            &device(&ui),
            &OrderDraft::market(OrderKind::Sell, "USD", 1, "cash"),
        )
        .await
        .unwrap();

    assert!(
        ui.calls()
            .contains(&"tap order.create.price.market".to_owned())
    );
}

/// The premium slider has no identifier, so a premium is refused rather
/// than silently published at zero.
#[tokio::test]
async fn a_premium_is_refused_rather_than_dropped() {
    let ui = app_with_pricing();
    adapter().onboard(&device(&ui), &env()).await.unwrap();
    let mut draft = OrderDraft::market(OrderKind::Sell, "USD", 1, "cash");
    draft.price = mortsom_app_contract::OrderPrice::Market { premium: 3 };

    let error = adapter()
        .create_order(&device(&ui), &draft)
        .await
        .unwrap_err();

    assert_eq!(
        error.category,
        mortsom_common::FailureCategory::AutomationContract
    );
    assert!(!ui.calls().contains(&"tap order.create.submit".to_owned()));
}

/// Taking a range order types the amount into the dialog the take button
/// opens and confirms it; the fixed take never sees a dialog.
#[tokio::test]
async fn taking_a_range_order_answers_the_amount_dialog_before_the_trade_opens() {
    let ui = app_with_pricing();
    adapter().onboard(&device(&ui), &env()).await.unwrap();
    let order = OrderReference::with_kind("o1", OrderKind::Sell);

    adapter()
        .find_and_take_order_at(&device(&ui), &order, 2)
        .await
        .unwrap();

    let calls = ui.calls();
    let asked = calls
        .iter()
        .position(|c| c == "type order.take.amount (1 chars)")
        .expect("the amount was typed");
    let confirmed = calls
        .iter()
        .position(|c| c == "tap order.take.amount.confirm")
        .expect("the amount was confirmed");
    let took = calls
        .iter()
        .position(|c| c == "tap order.take.confirm")
        .expect("the take button was pressed");
    assert!(took < asked && asked < confirmed, "{calls:?}");
}

/// A fixed order taken as if it were a range must not be taken at all: the
/// dialog never appears, and the amount would have gone nowhere.
#[tokio::test]
async fn a_fixed_order_refuses_a_take_amount() {
    let ui = app_with_orders();
    adapter().onboard(&device(&ui), &env()).await.unwrap();
    let order = OrderReference::with_kind("o1", OrderKind::Sell);

    let error = adapter()
        .find_and_take_order_at(&device(&ui), &order, 2)
        .await
        .unwrap_err();

    assert!(error.message.contains("order.take.amount"), "{error}");
}

/// Rating is the rate action, the chosen star and submit, and it ends back
/// on the same trade, which now reads rated.
#[tokio::test]
async fn rating_picks_the_star_submits_and_returns_to_the_rated_trade() {
    let ui = app_with_pricing();
    adapter().onboard(&device(&ui), &env()).await.unwrap();
    ui.present(&["trades.item.o1"]);
    ui.label(Ids::ORDER_STATUS, "pending-rating");
    let order = OrderReference::with_kind("o1", OrderKind::Sell);

    adapter()
        .rate_counterpart(&device(&ui), &order, 5)
        .await
        .unwrap();

    let calls = ui.calls();
    let star = calls
        .iter()
        .position(|c| c == "tap trade.rate.star.5")
        .expect("a star was tapped");
    let submit = calls
        .iter()
        .position(|c| c == "tap trade.rate.submit")
        .expect("the rating was submitted");
    assert!(star < submit, "{calls:?}");
    assert!(
        !calls.iter().any(|c| c == "tap trade.rate"),
        "the rating is on the trade itself: {calls:?}"
    );
    let shown = adapter().observe_order(&device(&ui), &order).await.unwrap();
    assert_eq!(shown.raw_label, "rated");
    assert_eq!(shown.state, OrderState::Completed);
}

/// A completed trade that asks for a rating offers it among its visible
/// actions: the submit on the trade detail is the rating action.
#[tokio::test]
async fn a_completed_trade_offers_its_rating_as_a_visible_action() {
    let ui = app_with_pricing();
    adapter().onboard(&device(&ui), &env()).await.unwrap();
    ui.present(&["trades.item.o1"]);
    ui.label(Ids::ORDER_STATUS, "pending-rating");
    let order = OrderReference::with_kind("o1", OrderKind::Sell);

    let actions = adapter()
        .visible_actions(&device(&ui), &order)
        .await
        .unwrap();

    assert!(actions.contains(&"rate".to_owned()), "{actions:?}");
}

/// The rating screen has five stars; anything else is a harness mistake,
/// caught before any control is pressed.
#[tokio::test]
async fn a_score_outside_the_five_stars_is_refused_before_any_tap() {
    let ui = app_with_pricing();
    adapter().onboard(&device(&ui), &env()).await.unwrap();
    let calls_before = ui.calls().len();
    let order = OrderReference::with_kind("o1", OrderKind::Sell);
    for score in [0, 6] {
        assert!(
            adapter()
                .rate_counterpart(&device(&ui), &order, score)
                .await
                .is_err()
        );
    }
    assert_eq!(ui.calls().len(), calls_before, "nothing was pressed");
}

/// The rating returning to a different trade is a contract failure: the
/// star would have been given on another order.
#[tokio::test]
async fn a_rating_that_lands_on_another_trade_is_a_contract_failure() {
    let ui = app_with_pricing();
    adapter().onboard(&device(&ui), &env()).await.unwrap();
    ui.present(&["trades.item.o1"]);
    ui.on_tap_relabel(Ids::TRADE_RATE_SUBMIT, Ids::ORDER_ID, "other");
    let order = OrderReference::with_kind("o1", OrderKind::Sell);

    let error = adapter()
        .rate_counterpart(&device(&ui), &order, 4)
        .await
        .unwrap_err();

    assert_eq!(
        error.category,
        mortsom_common::FailureCategory::AutomationContract
    );
}
