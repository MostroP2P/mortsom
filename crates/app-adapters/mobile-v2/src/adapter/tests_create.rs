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

/// A node that bonds makers answers the create with the pay-bond screen,
/// pushed over the new order's own: nothing is closed or paid there, and
/// the order is read from the trades list, parked on its bond.
#[tokio::test]
async fn a_create_on_a_node_that_bonds_makers_is_left_on_its_bond_screen() {
    let ui = app_with_orders();
    adapter().onboard(&device(&ui), &env()).await.unwrap();
    ui.on_tap(
        Ids::ORDER_CREATE_SUBMIT,
        &[Ids::BOND_ORDER_ID, Ids::BOND_CANCEL, Ids::APPBAR_BACK],
    );
    ui.on_tap_hide(
        Ids::ORDER_CREATE_SUBMIT,
        &[Ids::ORDER_ADD_FAB, Ids::NAV_ORDER_BOOK, Ids::NAV_TRADES],
    );
    ui.label(Ids::BOND_ORDER_ID, "o1");
    ui.on_tap(
        Ids::APPBAR_BACK,
        &[
            Ids::ORDER_ADD_FAB,
            Ids::NAV_ORDER_BOOK,
            Ids::NAV_TRADES,
            "trades.item.o1",
        ],
    );
    ui.on_tap_hide(
        Ids::APPBAR_BACK,
        &[Ids::BOND_ORDER_ID, Ids::BOND_CANCEL, Ids::APPBAR_BACK],
    );
    ui.on_tap("trades.item.o1", &[Ids::ORDER_STATUS, Ids::ORDER_ID]);
    ui.label(Ids::ORDER_STATUS, "waiting-bond");
    ui.label(Ids::ORDER_ID, "o1");

    let order = adapter()
        .create_order(&device(&ui), &draft())
        .await
        .unwrap();

    assert_eq!(order.id.as_deref(), Some("o1"));
    assert_eq!(order.state, OrderState::Taken);
    let calls = ui.calls();
    assert!(
        !calls
            .iter()
            .any(|c| c == "tap order.confirm.home" || c == "tap bond.cancel"),
        "{calls:?}"
    );
}
