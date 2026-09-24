//! The happy-path shapes against the fixture daemon: the book tags that
//! prove the pricing, the remainder of a range, and the rating proof.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use mortsom_scenario_kit::{cleanup::CleanupResult, registry};

use crate::flow_fixture::{Fixture, Misbehaviour, ORDER, REMAINDER};

fn scenario(name: &str) -> &'static registry::ScenarioDef {
    crate::link();
    registry::find(name).expect("registered")
}

/// The range is advertised with both bounds, taken at the middle, and
/// the remainder the daemon spawns is listed, pending, and closed by the
/// maker with relay confirmation.
#[tokio::test]
async fn a_range_order_is_taken_at_an_amount_and_its_remainder_is_closed() {
    let fixture = Fixture::new(false).await;

    (scenario("range_order").run)(&fixture.context)
        .await
        .unwrap();

    assert_eq!(
        fixture.actions(),
        [
            "create:alice",
            "take-at-20:bob",
            "take:bob",
            "invoice:bob",
            "hold:alice",
            "pay:alice",
            "fiat:bob",
            "release:alice",
            "cancel:alice"
        ]
    );
    assert_eq!(fixture.relay_states(REMAINDER), ["pending", "canceled"]);
    let cleanup = fixture.context.cleanup.report();
    assert!(cleanup.is_clean());
    assert_eq!(cleanup.entries.len(), 2, "the remainder was registered too");
    assert!(
        cleanup
            .entries
            .iter()
            .all(|e| e.result == Some(CleanupResult::Resolved))
    );
}

/// The mirror: Bob buys a range, Alice takes it as seller at the middle,
/// and the remainder is Bob's to list and close.
#[tokio::test]
async fn a_bought_range_leaves_its_remainder_to_the_buyer() {
    let fixture = Fixture::new(false).await;

    (scenario("range_buy_order").run)(&fixture.context)
        .await
        .unwrap();

    assert_eq!(
        fixture.actions(),
        [
            "create:bob",
            "take-at-20:alice",
            "take:alice",
            "hold:alice",
            "pay:alice",
            "invoice:bob",
            "fiat:bob",
            "release:alice",
            "cancel:bob"
        ]
    );
    assert_eq!(fixture.relay_states(REMAINDER), ["pending", "canceled"]);
    let cleanup = fixture.context.cleanup.report();
    assert!(cleanup.is_clean());
    assert_eq!(cleanup.entries[1].actor, mortsom_common::Actor::Bob);
}

/// A market order goes on the book without sats; the daemon prices it at
/// the take and the trade settles like any other.
#[tokio::test]
async fn a_market_order_is_published_unpriced_and_settles_once_priced() {
    let fixture = Fixture::new(false).await;

    (scenario("market_price_order").run)(&fixture.context)
        .await
        .unwrap();

    assert_eq!(
        fixture.relay_states(ORDER),
        ["pending", "in-progress", "success"]
    );
    assert_eq!(
        fixture.wallet_checks(),
        ["alice:pending", "alice:settled", "bob:settled"]
    );
    assert!(fixture.context.cleanup.report().is_clean());
}

/// An app that turns a market draft into a fixed order is caught at the
/// book, before anyone takes it.
#[tokio::test]
async fn a_market_order_published_with_sats_is_a_product_failure() {
    let fixture = Fixture::with(Misbehaviour {
        prices_market_orders_upfront: true,
        ..Misbehaviour::default()
    })
    .await;

    let error = (scenario("market_price_order").run)(&fixture.context)
        .await
        .unwrap_err();

    assert_eq!(error.category, mortsom_common::FailureCategory::Product);
    assert_eq!(fixture.actions(), ["create:alice"]);
}

/// After the trade both parties rate, each rating is followed by the
/// daemon's reputation event, and each app shows its trade as rated.
#[tokio::test]
async fn both_parties_rate_and_the_daemon_publishes_each_rating() {
    let fixture = Fixture::new(false).await;

    (scenario("full_trade_with_rating").run)(&fixture.context)
        .await
        .unwrap();

    let actions = fixture.actions();
    assert_eq!(
        &actions[actions.len() - 3..],
        ["release:alice", "rate:alice", "rate:bob"]
    );
    assert_eq!(fixture.ratings_published(), 2);
    assert!(fixture.context.cleanup.report().is_clean());
}

/// The maker's app dropping the remainder proves nothing on its own: the
/// daemon must publish it `canceled`.
#[tokio::test]
async fn a_remainder_cancel_the_daemon_never_publishes_fails_the_scenario() {
    let fixture = crate::flow_fixture::Fixture::with(crate::flow_fixture::Misbehaviour {
        never_publishes_cancel: true,
        ..crate::flow_fixture::Misbehaviour::default()
    })
    .await;

    let error = (scenario("range_order").run)(&fixture.context)
        .await
        .unwrap_err();

    assert_eq!(error.category, mortsom_common::FailureCategory::Relay);
    assert_eq!(fixture.relay_states(REMAINDER), ["pending"]);
}
