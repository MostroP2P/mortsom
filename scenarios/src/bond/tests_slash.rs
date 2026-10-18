//! The slashes the bond family proves against the fixture daemon, which
//! bonds both sides: every waiting-state timeout, by maker or taker, buyer
//! or seller; a solver slashing the seller's bond on a settle; a claim the
//! daemon cannot pay; and the bonds nobody pays, left to lapse.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use mortsom_common::{Actor, FailureCategory};
use mortsom_scenario_kit::registry;

use crate::flow_fixture::{BOND_POLICY, Fixture, IssuedInvoice, Misbehaviour, ORDER, SATS};

fn scenario(name: &str) -> &'static registry::ScenarioDef {
    crate::link();
    registry::find(name).expect("registered")
}

fn assert_settled_cleanly(fixture: &Fixture) {
    assert!(fixture.context.cleanup.report().is_clean());
    assert!(
        !fixture
            .context
            .budget
            .lock()
            .unwrap()
            .has_open_reservations()
    );
}

fn bond_of(fixture: &Fixture, actor: Actor) -> IssuedInvoice {
    fixture
        .bond_invoices()
        .into_iter()
        .find(|bond| bond.actor == actor)
        .unwrap_or_else(|| panic!("{actor} paid no bond"))
}

fn share() -> u64 {
    BOND_POLICY.counterparty_share_sats(BOND_POLICY.bond_sats(SATS))
}

/// The claim on `ORDER` went to `winner`, for the declared share, and was
/// paid.
fn assert_claim_paid_to(fixture: &Fixture, winner: Actor) {
    let claim = fixture.claim(ORDER).expect("a claim is offered");
    assert_eq!(claim.actor, winner);
    assert_eq!(claim.amount, share());
    assert_eq!(claim.phase, "completed");
}

#[tokio::test]
async fn a_seller_maker_who_never_funds_is_slashed_and_the_order_closes() {
    let fixture = Fixture::new(false).await.bonded().slashes_on_timeout();

    (scenario("seller_maker_bond_slashed_on_timeout").run)(&fixture.context)
        .await
        .unwrap();

    assert_eq!(
        fixture.actions(),
        [
            "create:alice",
            "bond:alice",
            "pay-bond:alice",
            "take:bob",
            "bond:bob",
            "pay-bond:bob",
            "invoice:bob",
            "daemon:slash-timeout",
            "claim:bob",
            "daemon:pay-claim",
        ]
    );
    assert_eq!(
        fixture.relay_states(ORDER),
        ["pending", "in-progress", "canceled"]
    );
    assert!(
        bond_of(&fixture, Actor::Alice).settled,
        "the maker's bond was slashed"
    );
    assert!(
        !bond_of(&fixture, Actor::Bob).settled,
        "the taker's bond came back"
    );
    assert_claim_paid_to(&fixture, Actor::Bob);
    assert_settled_cleanly(&fixture);
}

#[tokio::test]
async fn a_seller_taker_who_never_funds_is_slashed_and_the_buy_is_back_on_the_book() {
    let fixture = Fixture::new(false).await.bonded().slashes_on_timeout();

    (scenario("seller_taker_bond_slashed_on_timeout").run)(&fixture.context)
        .await
        .unwrap();

    assert_eq!(
        fixture.actions(),
        [
            "create:bob",
            "bond:bob",
            "pay-bond:bob",
            "take:alice",
            "bond:alice",
            "pay-bond:alice",
            "daemon:slash-timeout",
            "claim:bob",
            "daemon:pay-claim",
            "cancel:bob",
        ]
    );
    assert_eq!(
        fixture.relay_states(ORDER),
        ["pending", "in-progress", "pending", "canceled"]
    );
    assert!(
        bond_of(&fixture, Actor::Alice).settled,
        "the taker's bond was slashed"
    );
    assert!(
        !bond_of(&fixture, Actor::Bob).settled,
        "the maker's bond came back"
    );
    assert_claim_paid_to(&fixture, Actor::Bob);
    assert_settled_cleanly(&fixture);
}

#[tokio::test]
async fn a_buyer_maker_who_never_invoices_is_slashed_and_the_escrow_comes_back() {
    let fixture = Fixture::new(false).await.bonded().slashes_on_timeout();

    (scenario("buyer_maker_bond_slashed_on_timeout").run)(&fixture.context)
        .await
        .unwrap();

    assert_eq!(
        fixture.actions(),
        [
            "create:bob",
            "bond:bob",
            "pay-bond:bob",
            "take:alice",
            "bond:alice",
            "pay-bond:alice",
            "hold:alice",
            "pay:alice",
            "daemon:slash-timeout",
            "claim:alice",
            "daemon:pay-claim",
        ]
    );
    assert_eq!(
        fixture.relay_states(ORDER),
        ["pending", "in-progress", "canceled"]
    );
    assert!(
        bond_of(&fixture, Actor::Bob).settled,
        "the maker's bond was slashed"
    );
    assert!(
        !bond_of(&fixture, Actor::Alice).settled,
        "the taker's bond came back"
    );
    assert!(fixture.wallet_checks().contains(&"alice:failed".to_owned()));
    assert_claim_paid_to(&fixture, Actor::Alice);
    assert_settled_cleanly(&fixture);
}

/// A daemon that never slashes on a timeout leaves the order taken: the
/// scenario waits for the book to move and fails on the event that never
/// came, before anyone is offered a claim.
#[tokio::test]
async fn a_daemon_whose_clock_never_slashes_fails_the_timeout_scenario() {
    let fixture = Fixture::new(false).await.bonded();

    let error = (scenario("seller_maker_bond_slashed_on_timeout").run)(&fixture.context)
        .await
        .unwrap_err();

    assert_eq!(error.category, FailureCategory::Relay, "{error}");
    assert!(error.message.contains("cancelled"), "{error}");
    assert!(fixture.claim(ORDER).is_none());
}

#[tokio::test]
async fn a_solver_can_slash_the_seller_on_a_settle_and_the_buyer_claims() {
    let fixture = Fixture::new(false).await.bonded();

    (scenario("maker_bond_slashed_by_solver").run)(&fixture.context)
        .await
        .unwrap();

    let actions = fixture.actions();
    assert!(actions.contains(&"dispute:bob".to_owned()), "{actions:?}");
    assert!(
        actions.contains(&"solver:slash seller=true buyer=false".to_owned()),
        "{actions:?}"
    );
    assert_eq!(
        fixture.dispute_states("the-dispute"),
        ["initiated", "in-progress", "settled"]
    );
    assert_eq!(
        fixture.relay_states(ORDER),
        ["pending", "in-progress", "success"]
    );
    assert!(
        bond_of(&fixture, Actor::Alice).settled,
        "the seller's bond was slashed"
    );
    assert!(
        !bond_of(&fixture, Actor::Bob).settled,
        "the buyer's bond came back"
    );
    assert_claim_paid_to(&fixture, Actor::Bob);
    assert_settled_cleanly(&fixture);
}

#[tokio::test]
async fn a_claim_the_daemon_cannot_pay_is_asked_for_again_and_paid() {
    let fixture = Fixture::new(false).await.bonded().slashes_on_timeout();

    (scenario("bond_claim_fails_then_replaced").run)(&fixture.context)
        .await
        .unwrap();

    let actions = fixture.actions();
    let tail: Vec<&str> = actions
        .iter()
        .skip_while(|a| *a != "daemon:slash-timeout")
        .map(String::as_str)
        .collect();
    assert_eq!(
        tail,
        [
            "daemon:slash-timeout",
            "claim:alice",
            "daemon:pay-claim-held",
            "cancel-invoice:alice",
            "daemon:reprompt-claim",
            "claim:alice",
            "daemon:pay-claim",
            "cancel:alice",
        ]
    );
    assert_claim_paid_to(&fixture, Actor::Alice);
    let checks = fixture.wallet_checks();
    assert!(checks.contains(&"alice:failed".to_owned()), "{checks:?}");
    assert!(checks.contains(&"alice:settled".to_owned()), "{checks:?}");
    assert_settled_cleanly(&fixture);
}

#[tokio::test]
async fn an_unpaid_taker_bond_lapses_and_the_order_is_back_on_the_book() {
    let fixture = Fixture::new(false).await.bonded().expires_unpaid_bonds();

    (scenario("taker_bond_expires_unpaid").run)(&fixture.context)
        .await
        .unwrap();

    assert_eq!(
        fixture.actions(),
        [
            "create:alice",
            "bond:alice",
            "pay-bond:alice",
            "take:bob",
            "daemon:lapse-bond:bob",
            "cancel:alice",
        ]
    );
    assert_eq!(
        fixture.relay_states(ORDER),
        ["pending", "pending", "canceled"]
    );
    assert_eq!(fixture.bond_invoices().len(), 1, "Bob never read his bond");
    assert!(!bond_of(&fixture, Actor::Alice).settled);
    assert_settled_cleanly(&fixture);
}

#[tokio::test]
async fn an_unpaid_maker_bond_lapses_and_the_order_never_reaches_the_book() {
    let fixture = Fixture::new(false).await.bonded().expires_unpaid_bonds();

    (scenario("maker_bond_expires_unpaid").run)(&fixture.context)
        .await
        .unwrap();

    assert_eq!(
        fixture.actions(),
        ["create:alice", "daemon:lapse-bond:alice"]
    );
    assert!(fixture.relay_states(ORDER).is_empty());
    assert!(fixture.bond_invoices().is_empty());
    assert_settled_cleanly(&fixture);
}

/// A daemon that never lets an unpaid bond lapse leaves the maker parked
/// on it: the app keeps the order, and the scenario says so.
#[tokio::test]
async fn a_maker_bond_that_never_lapses_fails_the_scenario() {
    let fixture = Fixture::new(false).await.bonded();

    let error = (scenario("maker_bond_expires_unpaid").run)(&fixture.context)
        .await
        .unwrap_err();

    assert_eq!(error.category, FailureCategory::Product, "{error}");
}

/// The daemon tells the taker nothing when the bond lapses: an app that
/// keeps the take until the order itself closes fails on the taker, once
/// the maker's side is proven.
#[tokio::test]
async fn an_app_that_keeps_a_lapsed_take_fails_on_the_taker() {
    let fixture = Fixture::with(Misbehaviour {
        keeps_lapsed_takes: true,
        ..Default::default()
    })
    .await
    .bonded()
    .expires_unpaid_bonds();

    let error = (scenario("taker_bond_expires_unpaid").run)(&fixture.context)
        .await
        .unwrap_err();

    assert_eq!(error.category, FailureCategory::Product, "{error}");
    assert!(error.message.contains("still open on bob"), "{error}");
    assert!(
        fixture
            .actions()
            .contains(&"daemon:lapse-bond:bob".to_owned())
    );
}
