//! The expiration family against the fixture daemon: each scenario waits
//! for the daemon's clock, proves the published state after the baseline,
//! and accepts an app that dropped the closed trade.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use mortsom_scenario_kit::{cleanup::CleanupResult, registry};

use crate::flow_fixture::{Fixture, ORDER};

fn scenario(name: &str) -> &'static registry::ScenarioDef {
    crate::link();
    registry::find(name).expect("registered")
}

#[tokio::test]
async fn an_untaken_order_expires_on_the_daemons_clock() {
    let fixture = Fixture::new(false).await.expires_pending();

    (scenario("pending_order_expires").run)(&fixture.context)
        .await
        .unwrap();

    assert_eq!(fixture.actions(), ["create:alice"]);
    assert_eq!(fixture.relay_states(ORDER), ["pending", "expired"]);
    assert_eq!(
        fixture.context.cleanup.report().entries[0].result,
        Some(CleanupResult::Resolved)
    );
}

/// A daemon that never expires the order is reported at the deadline.
#[tokio::test]
async fn an_order_the_daemon_never_expires_fails_at_the_deadline() {
    let fixture = Fixture::new(false).await;

    let error = (scenario("pending_order_expires").run)(&fixture.context)
        .await
        .unwrap_err();

    assert_eq!(error.category, mortsom_common::FailureCategory::Relay);
}

/// The seller never pays: the daemon cancels, both apps drop the trade.
#[tokio::test]
async fn an_unpaid_hold_invoice_cancels_the_take_on_the_daemons_clock() {
    let fixture = Fixture::new(false)
        .await
        .reaps_abandoned_take_to("canceled");

    (scenario("hold_invoice_expires").run)(&fixture.context)
        .await
        .unwrap();

    assert_eq!(
        fixture.actions(),
        ["create:alice", "take:bob", "invoice:bob"]
    );
    assert_eq!(
        fixture.relay_states(ORDER),
        ["pending", "in-progress", "canceled"]
    );
    assert!(fixture.context.cleanup.report().is_clean());
}

/// The buyer never invoices a funded buy: the daemon cancels and the
/// seller's escrow comes back.
#[tokio::test]
async fn a_missing_buyer_invoice_cancels_the_funded_take_and_refunds() {
    let fixture = Fixture::new(false).await.cancels_funded_take();

    (scenario("buyer_invoice_deadline").run)(&fixture.context)
        .await
        .unwrap();

    assert_eq!(
        fixture.actions(),
        [
            "create:bob",
            "take:alice",
            "hold:alice",
            "pay:alice",
            "daemon:cancel-unpaid"
        ]
    );
    assert!(fixture.wallet_checks().contains(&"alice:failed".to_owned()));
    assert!(fixture.context.pending_holds.lock().unwrap().is_empty());
    assert!(fixture.context.cleanup.report().is_clean());
}
