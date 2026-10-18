//! The bond family against the fixture daemon, which bonds both sides:
//! bonds locked and returned, walked away from, slashed by the clock or
//! by a solver and claimed, or released by a ruling.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use mortsom_common::{Actor, FailureCategory};
use mortsom_scenario_kit::registry;

use crate::flow_fixture::{BOND_POLICY, Fixture, Misbehaviour, ORDER, SATS};

fn scenario(name: &str) -> &'static registry::ScenarioDef {
    crate::link();
    registry::find(name).expect("registered")
}

/// Every reservation resolved and no order left for the sweep.
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

#[tokio::test]
async fn both_bonds_lock_the_trade_completes_and_both_come_back() {
    let fixture = Fixture::new(false).await.bonded();

    (scenario("taker_bond_released_after_trade").run)(&fixture.context)
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
            "hold:alice",
            "pay:alice",
            "fiat:bob",
            "release:alice",
        ]
    );
    // The order reaches the book once the maker's bond locks, the take
    // once the taker's does.
    assert_eq!(
        fixture.relay_states(ORDER),
        ["pending", "in-progress", "success"]
    );
    let bonds = fixture.bond_invoices();
    assert_eq!(bonds.len(), 2);
    assert!(
        bonds.iter().all(|bond| !bond.settled),
        "returned, not spent"
    );
    assert!(fixture.wallet_checks().contains(&"bob:failed".to_owned()));
    assert_settled_cleanly(&fixture);
}

/// Without `stack up --bond` the daemon asks for no bond: every scenario
/// says so before an order exists.
#[tokio::test]
async fn a_stack_without_a_bond_ends_the_family_as_environment_before_any_order() {
    let fixture = Fixture::new(false).await;

    for def in registry::all()
        .into_iter()
        .filter(|def| def.tags.contains(&"bond"))
    {
        let error = (def.run)(&fixture.context).await.unwrap_err();
        assert_eq!(
            error.category,
            FailureCategory::EnvironmentMisconfiguration,
            "{}",
            def.name
        );
        assert!(
            error.message.contains("stack up --bond"),
            "{}: {error}",
            def.name
        );
    }
    assert!(fixture.actions().is_empty());
}

/// A daemon that publishes the take while the bond is outstanding lets a
/// taker who never pays block the order for everyone.
#[tokio::test]
async fn a_daemon_that_publishes_the_take_before_the_bond_locks_fails() {
    let fixture = Fixture::with(Misbehaviour {
        publishes_take_before_bond_lock: true,
        ..Default::default()
    })
    .await
    .bonded();

    let error = (scenario("taker_bond_released_after_trade").run)(&fixture.context)
        .await
        .unwrap_err();

    assert_eq!(error.category, FailureCategory::Product);
    assert!(error.message.contains("as taken"), "{error}");
}

/// A daemon that keeps an honest taker's bond is a product failure, and
/// the sats it kept are accounted before the failure is reported.
#[tokio::test]
async fn a_daemon_that_slashes_an_honest_bond_fails_with_the_sats_accounted() {
    let fixture = Fixture::with(Misbehaviour {
        slashes_honest_bonds: true,
        ..Default::default()
    })
    .await
    .bonded();

    let error = (scenario("taker_bond_released_after_trade").run)(&fixture.context)
        .await
        .unwrap_err();

    assert_eq!(error.category, FailureCategory::Product);
    assert!(error.message.contains("slashed"), "{error}");
    let budget = fixture.context.budget.lock().unwrap();
    assert!(budget.totals().settled_sats > 0, "the bond is on the books");
}

#[tokio::test]
async fn walking_away_from_the_bond_pays_nothing_and_leaves_the_order_in_the_book() {
    let fixture = Fixture::new(false).await.bonded();

    (scenario("taker_bond_declined").run)(&fixture.context)
        .await
        .unwrap();

    assert_eq!(
        fixture.actions(),
        [
            "create:alice",
            "bond:alice",
            "pay-bond:alice",
            "take:bob",
            "decline-bond:bob",
            "cancel:alice",
        ]
    );
    assert_eq!(fixture.relay_states(ORDER), ["pending", "canceled"]);
    let bonds = fixture.bond_invoices();
    assert_eq!(bonds.len(), 1, "only the maker's bond was ever paid");
    assert_eq!(bonds[0].actor, Actor::Alice);
    assert!(!bonds[0].settled);
    assert_settled_cleanly(&fixture);
}

#[tokio::test]
async fn the_seller_who_takes_a_buy_pays_the_bond_and_then_the_escrow() {
    let fixture = Fixture::new(false).await.bonded();

    (scenario("seller_taker_bond_then_escrow").run)(&fixture.context)
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
            "invoice:bob",
            "fiat:bob",
            "release:alice",
        ]
    );
    assert_eq!(
        fixture.relay_states(ORDER),
        ["pending", "in-progress", "success"]
    );
    let checks = fixture.wallet_checks();
    assert!(checks.contains(&"alice:settled".to_owned()));
    assert!(checks.contains(&"alice:failed".to_owned()));
    assert_settled_cleanly(&fixture);
}

#[tokio::test]
async fn the_makers_bond_gates_publication_and_comes_back_on_cancel() {
    let fixture = Fixture::new(false).await.bonded();

    (scenario("maker_bond_locked_before_publish").run)(&fixture.context)
        .await
        .unwrap();

    assert_eq!(
        fixture.actions(),
        [
            "create:alice",
            "bond:alice",
            "pay-bond:alice",
            "cancel:alice",
        ]
    );
    assert_eq!(fixture.relay_states(ORDER), ["pending", "canceled"]);
    assert!(fixture.wallet_checks().contains(&"alice:failed".to_owned()));
    assert_settled_cleanly(&fixture);
}

#[tokio::test]
async fn an_abandoned_maker_bond_publishes_nothing_and_charges_nothing() {
    let fixture = Fixture::new(false).await.bonded();

    (scenario("maker_bond_abandoned").run)(&fixture.context)
        .await
        .unwrap();

    assert_eq!(fixture.actions(), ["create:alice", "decline-bond:alice"]);
    assert!(fixture.relay_states(ORDER).is_empty());
    assert!(fixture.bond_invoices().is_empty());
    assert_settled_cleanly(&fixture);
}

#[tokio::test]
async fn a_silent_taker_is_slashed_by_the_clock_and_the_maker_claims_the_share() {
    let fixture = Fixture::new(false).await.bonded().slashes_on_timeout();

    (scenario("buyer_taker_bond_slashed_on_timeout").run)(&fixture.context)
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
            "daemon:slash-timeout",
            "claim:alice",
            "daemon:pay-claim",
            "cancel:alice",
        ]
    );
    assert_eq!(
        fixture.relay_states(ORDER),
        ["pending", "in-progress", "pending", "canceled"]
    );
    let bonds = fixture.bond_invoices();
    let bob = bonds.iter().find(|b| b.actor == Actor::Bob).unwrap();
    let alice = bonds.iter().find(|b| b.actor == Actor::Alice).unwrap();
    assert!(bob.settled, "the slashed bond settled");
    assert!(!alice.settled, "the maker's bond came back");
    assert_eq!(fixture.claim(ORDER).unwrap().phase, "completed");
    assert_eq!(
        fixture.claim(ORDER).unwrap().amount,
        BOND_POLICY.counterparty_share_sats(BOND_POLICY.bond_sats(SATS))
    );
    assert_settled_cleanly(&fixture);
}

/// A daemon that offers the winner less than the declared split is a
/// product failure caught before any invoice is sent.
#[tokio::test]
async fn a_claim_below_the_declared_share_is_refused() {
    let fixture = Fixture::new(false).await.bonded().slashes_on_timeout();
    let mut ctx_policy = fixture.context.limits.bond.unwrap();
    ctx_policy.node_share_bps = 2500;
    let mut context = fixture.context;
    context.limits.bond = Some(ctx_policy);

    let error = (scenario("buyer_taker_bond_slashed_on_timeout").run)(&context)
        .await
        .unwrap_err();

    assert_eq!(error.category, FailureCategory::Product);
    assert!(error.message.contains("leaves the winner"), "{error}");
}

#[tokio::test]
async fn a_solver_can_slash_the_buyer_and_the_seller_claims() {
    let fixture = Fixture::new(false).await.bonded();

    (scenario("taker_bond_slashed_by_solver").run)(&fixture.context)
        .await
        .unwrap();

    let actions = fixture.actions();
    assert!(
        actions.contains(&"solver:slash seller=false buyer=true".to_owned()),
        "{actions:?}"
    );
    assert!(actions.contains(&"daemon:slash-dispute".to_owned()));
    assert!(actions.contains(&"claim:alice".to_owned()));
    assert_eq!(
        fixture.dispute_states("the-dispute"),
        ["initiated", "in-progress", "seller-refunded"]
    );
    assert_eq!(
        fixture.relay_states(ORDER),
        ["pending", "in-progress", "canceled"]
    );
    let bonds = fixture.bond_invoices();
    assert!(
        bonds
            .iter()
            .find(|b| b.actor == Actor::Bob)
            .unwrap()
            .settled
    );
    assert!(
        !bonds
            .iter()
            .find(|b| b.actor == Actor::Alice)
            .unwrap()
            .settled
    );
    assert_eq!(fixture.claim(ORDER).unwrap().phase, "completed");
    assert_settled_cleanly(&fixture);
}

#[tokio::test]
async fn a_ruling_that_slashes_nobody_releases_both_bonds() {
    let fixture = Fixture::new(false).await.bonded();

    (scenario("bonds_released_by_solver_ruling").run)(&fixture.context)
        .await
        .unwrap();

    let actions = fixture.actions();
    assert!(
        actions.contains(&"solver:slash seller=false buyer=false".to_owned()),
        "{actions:?}"
    );
    assert!(!actions.iter().any(|a| a.starts_with("daemon:slash")));
    assert_eq!(
        fixture.relay_states(ORDER),
        ["pending", "in-progress", "success"]
    );
    assert!(fixture.bond_invoices().iter().all(|b| !b.settled));
    assert!(fixture.claim(ORDER).is_none());
    assert_settled_cleanly(&fixture);
}
