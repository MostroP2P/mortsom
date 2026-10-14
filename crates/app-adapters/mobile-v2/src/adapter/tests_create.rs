//! A new order opens on its own screen, whose close pops back to the book.
//! The book draws a moment after that screen leaves, and until it does the
//! closing screen's back is still up for a look to find and lose.
use super::*;

#[tokio::test]
async fn closing_the_new_order_waits_for_the_book_it_returns_to() {
    let ui = app_with_orders();
    adapter().onboard(&device(&ui), &env()).await.unwrap();
    // The new order's screen covers the book, its navigation included.
    ui.on_tap(
        Ids::ORDER_CREATE_SUBMIT,
        &[Ids::ORDER_CONFIRM_HOME, Ids::APPBAR_BACK],
    );
    ui.on_tap_hide(
        Ids::ORDER_CREATE_SUBMIT,
        &[Ids::ORDER_ADD_FAB, Ids::NAV_ORDER_BOOK, Ids::NAV_TRADES],
    );
    // Closing it pops back to the book, drawn a few looks later; the back
    // answers one more look and leaves with the screen.
    ui.on_tap(Ids::ORDER_CONFIRM_HOME, &["trades.item.o1"]);
    ui.on_tap_draws_after(
        Ids::ORDER_CONFIRM_HOME,
        &[Ids::ORDER_ADD_FAB, Ids::NAV_ORDER_BOOK, Ids::NAV_TRADES],
        2,
    );
    ui.vanishes_after(Ids::APPBAR_BACK, 1);

    let order = adapter()
        .create_order(&device(&ui), &draft())
        .await
        .unwrap();

    assert_eq!(order.id.as_deref(), Some("o1"));
    let calls = ui.calls();
    let closed = calls
        .iter()
        .position(|c| c == "tap order.confirm.home")
        .expect("closed");
    assert!(
        !calls[closed..].contains(&"tap appbar.back".to_owned()),
        "{calls:?}"
    );
}
