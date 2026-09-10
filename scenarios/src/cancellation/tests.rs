//! The cancellation family against the fixture daemon: who closes what,
//! what the relay must and must not show, and what the escrow does.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use mortsom_android_driver::avd::Actor;
use mortsom_scenario_kit::{cleanup::CleanupResult, registry};

use crate::flow_fixture::{Fixture, Misbehaviour, ORDER};

fn scenario(name: &str) -> &'static registry::ScenarioDef {
    crate::link();
    registry::find(name).expect("registered")
}

/// The maker closes a pending order alone: one cancel, the book agrees, no
/// payment was ever made.
#[tokio::test]
async fn cancel_before_taken_is_the_makers_alone_to_close() {
    let fixture = Fixture::new(false).await;

    (scenario("cancel_before_taken").run)(&fixture.context)
        .await
        .unwrap();

    assert_eq!(fixture.actions(), ["create:alice", "cancel:alice"]);
    assert_eq!(fixture.relay_states(ORDER), ["pending", "canceled"]);
    assert!(fixture.wallet_checks().is_empty(), "no payment to look up");
    let cleanup = fixture.context.cleanup.report();
    assert!(cleanup.is_clean());
    assert_eq!(cleanup.entries[0].result, Some(CleanupResult::Resolved));
}

/// An abandoned take is reaped by the daemon: back to pending, which the
/// scenario leaves open for the cleanup sweep, or closed, which it resolves.
#[tokio::test]
async fn an_abandoned_take_reaped_back_to_pending_is_left_for_the_sweep() {
    let fixture = Fixture::new(false).await.reaps_abandoned_take_to("pending");

    (scenario("cancel_after_take_unpaid").run)(&fixture.context)
        .await
        .unwrap();

    assert_eq!(fixture.actions(), ["create:alice", "take:bob"]);
    assert_eq!(
        fixture.relay_states(ORDER),
        ["pending", "in-progress", "pending"]
    );
    let cleanup = fixture.context.cleanup.report();
    assert_eq!(cleanup.entries[0].actor, Actor::Alice);
    assert!(
        cleanup.entries[0].result.is_none(),
        "the republished order is the sweep's to close"
    );
}

#[tokio::test]
async fn an_abandoned_take_the_daemon_closes_is_resolved() {
    let fixture = Fixture::new(false)
        .await
        .reaps_abandoned_take_to("canceled");

    (scenario("cancel_after_take_unpaid").run)(&fixture.context)
        .await
        .unwrap();

    assert_eq!(
        fixture.context.cleanup.report().entries[0].result,
        Some(CleanupResult::Resolved)
    );
}

/// A daemon that never reaps the take is a product failure, reported when
/// the expiration deadline passes rather than by waiting forever.
#[tokio::test]
async fn a_take_the_daemon_never_reaps_fails_at_the_expiration_deadline() {
    let fixture = Fixture::new(false).await;

    let error = (scenario("cancel_after_take_unpaid").run)(&fixture.context)
        .await
        .unwrap_err();

    assert_eq!(error.category, mortsom_common::FailureCategory::Relay);
    assert!(error.message.contains("after the baseline"), "{error}");
}

/// The taker's own cancel puts the order back on the book, proved by a
/// `pending` published after the take, not the original one.
#[tokio::test]
async fn a_taker_cancel_republishes_the_order_for_the_maker() {
    let fixture = Fixture::new(false).await;

    (scenario("taker_cancel_at_taken").run)(&fixture.context)
        .await
        .unwrap();

    assert_eq!(
        fixture.actions(),
        ["create:alice", "take:bob", "invoice:bob", "cancel:bob"]
    );
    assert_eq!(
        fixture.relay_states(ORDER),
        ["pending", "in-progress", "pending"]
    );
    assert!(fixture.context.cleanup.report().entries[0].result.is_none());
}

/// Both sides cancel a funded order: the escrow comes back (the seller's
/// hold payment fails) and the order is resolved.
#[tokio::test]
async fn a_cooperative_cancel_returns_the_escrow() {
    let fixture = Fixture::new(false).await;

    (scenario("cooperative_cancel").run)(&fixture.context)
        .await
        .unwrap();

    assert_eq!(
        fixture.actions(),
        [
            "create:alice",
            "take:bob",
            "invoice:bob",
            "hold:alice",
            "pay:alice",
            "cancel:alice",
            "cancel:bob"
        ]
    );
    assert_eq!(fixture.relay_states(ORDER).last().unwrap(), "canceled");
    assert!(fixture.wallet_checks().contains(&"alice:failed".to_owned()));
    assert!(fixture.context.pending_holds.lock().unwrap().is_empty());
    assert!(fixture.context.cleanup.report().is_clean());
}

/// A lone cancel of a funded order must not close it. The well-behaved
/// daemon passes: the buyer stays active and nothing is published during
/// the window, then the buyer's agreement returns the escrow.
#[tokio::test]
async fn a_lone_cancel_that_the_daemon_ignores_passes_and_the_escrow_returns_afterwards() {
    let fixture = Fixture::new(false).await;

    (scenario("unilateral_cancel_rejected").run)(&fixture.context)
        .await
        .unwrap();

    let actions = fixture.actions();
    assert_eq!(
        &actions[actions.len() - 2..],
        ["cancel:alice", "cancel:bob"]
    );
    assert!(fixture.wallet_checks().contains(&"alice:failed".to_owned()));
    assert!(fixture.context.cleanup.report().is_clean());
}

/// A daemon that cancels a funded order on one side's word is the product
/// failure this scenario exists to catch.
#[tokio::test]
async fn a_daemon_that_closes_on_a_lone_cancel_fails_the_scenario() {
    let fixture = Fixture::with(Misbehaviour {
        lone_cancel_closes: true,
        ..Misbehaviour::default()
    })
    .await;

    let error = (scenario("unilateral_cancel_rejected").run)(&fixture.context)
        .await
        .unwrap_err();

    assert_eq!(error.category, mortsom_common::FailureCategory::Product);
    assert!(
        !fixture.actions().contains(&"cancel:bob".to_owned()),
        "the buyer never accepted: {:?}",
        fixture.actions()
    );
}
