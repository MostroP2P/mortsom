//! My Trades is read only once it says it loaded: rows, `trades.empty`, or
//! `trades.error`. A list still loading has no rows either, and reading it
//! as empty would pass a listed trade for one that is gone.
use super::*;

/// The onboarded app, with My Trades behaving as `on_open` says once
/// `nav.trades` is tapped.
async fn trades_app(on_open: &[&str]) -> FakeUi {
    let ui = app_with_orders();
    adapter().onboard(&device(&ui), &env()).await.unwrap();
    // Opening the list shows `on_open` in place of the empty state.
    let replaced: Vec<&str> = [Ids::TRADES_EMPTY]
        .into_iter()
        .filter(|id| !on_open.contains(id))
        .collect();
    ui.on_tap_hide(Ids::NAV_TRADES, &replaced);
    ui.on_tap(Ids::NAV_TRADES, on_open);
    ui
}

#[tokio::test]
async fn a_loaded_empty_list_lists_no_orders() {
    let ui = trades_app(&[Ids::TRADES_EMPTY]).await;

    let orders = adapter().owned_orders(&device(&ui)).await.unwrap();

    assert!(orders.is_empty());
}

#[tokio::test]
async fn a_list_still_loading_is_never_read_as_empty() {
    let ui = trades_app(&[]).await;

    let error = adapter().owned_orders(&device(&ui)).await.unwrap_err();

    assert_eq!(
        error.category,
        mortsom_common::FailureCategory::AutomationContract
    );
    assert!(error.message.contains(Ids::TRADES_EMPTY), "{error}");
}

#[tokio::test]
async fn a_list_that_failed_to_load_is_the_products_failure() {
    let ui = trades_app(&[Ids::TRADES_ERROR]).await;

    let error = adapter().owned_orders(&device(&ui)).await.unwrap_err();

    assert_eq!(error.category, mortsom_common::FailureCategory::Product);
    assert!(error.message.contains(Ids::TRADES_ERROR), "{error}");
}

/// The list may finish loading after the first read: the rows it then
/// shows are the answer, not the empty read before them.
#[tokio::test]
async fn rows_that_arrive_after_the_first_read_are_listed() {
    let ui = trades_app(&["trades.item.o1"]).await;
    ui.appears_after("trades.item.o1", 2);

    let orders = adapter().owned_orders(&device(&ui)).await.unwrap();

    assert_eq!(orders, vec!["o1".to_owned()]);
}
