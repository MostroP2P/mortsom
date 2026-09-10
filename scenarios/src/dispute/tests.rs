//! The dispute family against the fixture daemon: each scenario proves
//! the dispute events the daemon publishes after each step and where the
//! escrow ends up.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use mortsom_scenario_kit::registry;

use crate::flow_fixture::{DISPUTE, Fixture, ORDER};

fn scenario(name: &str) -> &'static registry::ScenarioDef {
    crate::link();
    registry::find(name).expect("registered")
}

#[tokio::test]
async fn a_solver_settles_the_buyers_dispute_and_the_payout_lands() {
    let fixture = Fixture::new(false).await;

    (scenario("dispute_settled_by_solver").run)(&fixture.context)
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
            "fiat:bob",
            "dispute:bob",
            "solver:take",
            "solver:settle"
        ]
    );
    assert_eq!(
        fixture.dispute_states(DISPUTE),
        ["initiated", "in-progress", "settled"]
    );
    assert_eq!(
        fixture.relay_states(ORDER),
        ["pending", "in-progress", "success"]
    );
    assert!(
        fixture
            .wallet_checks()
            .iter()
            .any(|check| check == "bob:settled")
    );
    assert!(fixture.context.cleanup.report().is_clean());
}

#[tokio::test]
async fn a_solver_cancels_the_sellers_dispute_and_the_escrow_returns() {
    let fixture = Fixture::new(false).await;

    (scenario("dispute_canceled_by_solver").run)(&fixture.context)
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
            "dispute:alice",
            "solver:take",
            "solver:cancel"
        ]
    );
    assert_eq!(
        fixture.dispute_states(DISPUTE),
        ["initiated", "in-progress", "seller-refunded"]
    );
    assert_eq!(
        fixture.relay_states(ORDER),
        ["pending", "in-progress", "canceled"]
    );
    assert!(fixture.wallet_checks().contains(&"alice:failed".to_owned()));
    assert!(fixture.context.cleanup.report().is_clean());
}

#[tokio::test]
async fn a_seller_releasing_under_dispute_completes_the_trade_and_closes_the_dispute() {
    let fixture = Fixture::new(false).await;

    (scenario("dispute_released_by_seller").run)(&fixture.context)
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
            "fiat:bob",
            "dispute:bob",
            "release:alice"
        ]
    );
    assert_eq!(fixture.dispute_states(DISPUTE), ["initiated", "settled"]);
    assert_eq!(
        fixture.relay_states(ORDER),
        ["pending", "in-progress", "success"]
    );
    assert!(fixture.context.cleanup.report().is_clean());
}

/// A daemon that credits the dispute to the wrong side is a product
/// failure, not a passing dispute.
#[tokio::test]
async fn a_dispute_the_daemon_credits_to_the_wrong_side_fails() {
    let fixture = Fixture::with(crate::flow_fixture::Misbehaviour {
        swaps_dispute_initiator: true,
        ..Default::default()
    })
    .await;

    let error = (scenario("dispute_canceled_by_solver").run)(&fixture.context)
        .await
        .unwrap_err();

    assert_eq!(error.category, mortsom_common::FailureCategory::Product);
    assert!(error.message.contains("opened by the buyer"), "{error}");
}

/// A run without a solver refuses the scenario as an environment error
/// before anything is taken.
#[tokio::test]
async fn a_run_without_a_solver_cannot_rule_on_a_dispute() {
    let fixture = Fixture::new(false).await.without_solver();

    let error = (scenario("dispute_settled_by_solver").run)(&fixture.context)
        .await
        .unwrap_err();

    assert_eq!(
        error.category,
        mortsom_common::FailureCategory::EnvironmentMisconfiguration
    );
    assert!(fixture.actions().contains(&"dispute:bob".to_owned()));
    assert!(!fixture.actions().contains(&"solver:take".to_owned()));
}

/// The solver's answer names the order; a dispute about some other order
/// is not the one the scenario opened.
#[tokio::test]
async fn a_dispute_about_another_order_is_not_taken_as_ours() {
    let fixture = Fixture::with(crate::flow_fixture::Misbehaviour {
        solver_reports_another_order: true,
        ..Default::default()
    })
    .await;

    let error = (scenario("dispute_settled_by_solver").run)(&fixture.context)
        .await
        .unwrap_err();

    assert_eq!(error.category, mortsom_common::FailureCategory::Product);
    assert!(error.message.contains("is about order"), "{error}");
}
