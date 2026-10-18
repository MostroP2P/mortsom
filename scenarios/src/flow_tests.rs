#![allow(clippy::unwrap_used, clippy::expect_used)]

use crate::flow_fixture::{BOND_POLICY, Fixture, SATS};
use mortsom_app_contract::OrderKind;
use mortsom_common::Actor;
use mortsom_scenario_kit::{cleanup::CleanupResult, registry};

#[tokio::test]
async fn buy_maker_invoices_only_after_the_seller_hold_is_accepted() {
    crate::link();
    let definition = registry::find("happy_buy").expect("happy_buy must be registered");
    assert!(definition.moves_value);
    assert!(definition.tags.contains(&"smoke") && definition.tags.contains(&"happy-path"));
    let fixture = Fixture::new(false).await;

    (definition.run)(&fixture.context).await.unwrap();

    assert_eq!(fixture.adapter.created(), (Actor::Bob, OrderKind::Buy));
    assert_eq!(
        fixture.actions(),
        [
            "create:bob",
            "take:alice",
            "hold:alice",
            "pay:alice",
            "invoice:bob",
            "fiat:bob",
            "release:alice"
        ]
    );
    assert!(fixture.context.pending_holds.lock().unwrap().is_empty());
    let cleanup = fixture.context.cleanup.report();
    assert!(cleanup.is_clean());
    assert_eq!(cleanup.entries[0].actor, Actor::Bob);
    assert_eq!(cleanup.entries[0].result, Some(CleanupResult::Resolved));
    assert_eq!(
        fixture.wallet_checks(),
        ["alice:pending", "alice:settled", "bob:settled"]
    );
}

#[tokio::test]
async fn failed_seller_payment_never_requests_the_buyers_invoice_or_releases_the_order() {
    crate::link();
    let definition = registry::find("happy_buy").expect("happy_buy must be registered");
    let fixture = Fixture::new(true).await;
    assert!((definition.run)(&fixture.context).await.is_err());
    assert_eq!(
        fixture.actions(),
        ["create:bob", "take:alice", "hold:alice", "pay:alice"]
    );
    assert!(!fixture.context.cleanup.report().is_clean());
}

#[tokio::test]
async fn buy_flow_respects_the_existing_spend_cap_before_payment() {
    crate::link();
    let definition = registry::find("happy_buy").expect("happy_buy must be registered");
    let mut fixture = Fixture::new(false).await;
    // One sat over what an order may hold: escrow plus both bonds.
    fixture.context.amount_sats = SATS + 2 * BOND_POLICY.bond_sats(SATS) + 1;
    assert!((definition.run)(&fixture.context).await.is_err());
    assert_eq!(fixture.actions(), ["create:bob", "take:alice"]);
}

#[tokio::test]
async fn missing_payout_hash_prevents_release_and_resolution() {
    let fixture = Fixture::new(false).await;
    let mut taken = super::create_buy_and_fund(&fixture.context).await.unwrap();
    super::fiat_sent(&fixture.context, &taken.order)
        .await
        .unwrap();
    taken.payout_hash = None;
    let result = super::release_and_settle(&fixture.context, &taken).await;
    assert!(result.is_err(), "missing payout evidence must fail closed");
    assert!(!fixture.actions().contains(&"release:alice".to_owned()));
    assert!(!fixture.context.cleanup.report().is_clean());
}
