//! The catalogue as the operator selects it (spec §8): families by tag,
//! with `smoke` cutting across them.
#![allow(clippy::unwrap_used)]

use std::collections::BTreeSet;

use mortsom_scenario_kit::registry;

fn tagged(tag: &str) -> BTreeSet<&'static str> {
    crate::link();
    registry::select(&[], Some(tag))
        .unwrap()
        .into_iter()
        .map(|def| def.name)
        .collect()
}

#[test]
fn the_happy_path_family_is_every_shape_of_a_trade_that_works() {
    assert_eq!(
        tagged("happy-path"),
        BTreeSet::from([
            "happy_sell",
            "happy_buy",
            "range_order",
            "range_buy_order",
            "market_price_order",
            "full_trade_with_rating",
        ])
    );
}

#[test]
fn the_cancellation_family_is_every_way_an_order_ends_before_the_sats_move() {
    assert_eq!(
        tagged("cancellation"),
        BTreeSet::from([
            "cancel_before_taken",
            "cancel_after_take_unpaid",
            "taker_cancel_at_taken",
            "cooperative_cancel",
            "unilateral_cancel_rejected",
        ])
    );
}

#[test]
fn the_expiration_family_is_every_clock_the_daemon_runs_on_its_own() {
    assert_eq!(
        tagged("expiration"),
        BTreeSet::from([
            "pending_order_expires",
            "hold_invoice_expires",
            "buyer_invoice_deadline",
        ])
    );
    crate::link();
    assert!(registry::find("expiration_unpaid").is_none());
}

#[test]
fn the_dispute_family_is_every_way_a_dispute_ends() {
    assert_eq!(
        tagged("dispute"),
        BTreeSet::from([
            "dispute_settled_by_solver",
            "dispute_canceled_by_solver",
            "dispute_released_by_seller",
        ])
    );
    crate::link();
    let needs_solver: BTreeSet<&str> = registry::all()
        .into_iter()
        .filter(|def| def.needs_solver)
        .map(|def| def.name)
        .collect();
    assert_eq!(
        needs_solver,
        BTreeSet::from([
            "dispute_settled_by_solver",
            "dispute_canceled_by_solver",
            "taker_bond_slashed_by_solver",
            "maker_bond_slashed_by_solver",
            "bonds_released_by_solver_ruling",
        ]),
        "only a solver's ruling needs a solver"
    );
}

#[test]
fn the_payment_failure_family_is_what_the_daemon_cannot_pay_and_what_one_payee_can_pin() {
    assert_eq!(
        tagged("payment-failure"),
        BTreeSet::from([
            "payout_fails_then_replaced",
            "payout_invoice_below_expiry_floor",
            "payout_slots_capped_per_destination",
        ])
    );
    crate::link();
    // Only the refused invoice ends before the sats move.
    let moving: BTreeSet<&str> = registry::all()
        .into_iter()
        .filter(|def| def.tags.contains(&"payment-failure"))
        .filter(|def| def.moves_value)
        .map(|def| def.name)
        .collect();
    assert_eq!(
        moving,
        BTreeSet::from([
            "payout_fails_then_replaced",
            "payout_slots_capped_per_destination"
        ])
    );
}

#[test]
fn the_bond_family_releases_slashes_and_claims_every_bond_two_actors_can_reach() {
    assert_eq!(
        tagged("bond"),
        BTreeSet::from([
            // Released.
            "taker_bond_released_after_trade",
            "seller_taker_bond_then_escrow",
            "taker_bond_declined",
            "taker_bond_expires_unpaid",
            "maker_bond_locked_before_publish",
            "maker_bond_abandoned",
            "maker_bond_expires_unpaid",
            "bonds_released_by_solver_ruling",
            // Slashed by the clock, whoever owed the step, and claimed.
            "buyer_taker_bond_slashed_on_timeout",
            "seller_maker_bond_slashed_on_timeout",
            "seller_taker_bond_slashed_on_timeout",
            "buyer_maker_bond_slashed_on_timeout",
            // Slashed by a solver, either side, and claimed.
            "taker_bond_slashed_by_solver",
            "maker_bond_slashed_by_solver",
            // A claim the daemon cannot pay, asked for again.
            "bond_claim_fails_then_replaced",
        ])
    );
    crate::link();
    // Only a maker bond nobody pays costs nothing at all.
    let still: BTreeSet<&str> = registry::all()
        .into_iter()
        .filter(|def| def.tags.contains(&"bond"))
        .filter(|def| !def.moves_value)
        .map(|def| def.name)
        .collect();
    assert_eq!(
        still,
        BTreeSet::from(["maker_bond_abandoned", "maker_bond_expires_unpaid"])
    );
    let solver: BTreeSet<&str> = registry::all()
        .into_iter()
        .filter(|def| def.tags.contains(&"bond") && def.needs_solver)
        .map(|def| def.name)
        .collect();
    assert_eq!(
        solver,
        BTreeSet::from([
            "taker_bond_slashed_by_solver",
            "maker_bond_slashed_by_solver",
            "bonds_released_by_solver_ruling"
        ])
    );
}

/// Three scenarios, enough to tell a broken setup from a working one.
#[test]
fn smoke_cuts_across_the_families() {
    assert_eq!(
        tagged("smoke"),
        BTreeSet::from(["happy_sell", "happy_buy", "cancel_before_taken"])
    );
}

/// Only the scenarios that fund an order move value; the ones that end
/// before the sats move say so, and never need mainnet clearance.
#[test]
fn value_moves_only_where_the_hold_invoice_is_paid() {
    crate::link();
    let moving: BTreeSet<&str> = registry::all()
        .into_iter()
        .filter(|def| def.tags.contains(&"cancellation"))
        .filter(|def| def.moves_value)
        .map(|def| def.name)
        .collect();
    assert_eq!(
        moving,
        BTreeSet::from(["cooperative_cancel", "unilateral_cancel_rejected"])
    );
    assert!(
        registry::all()
            .into_iter()
            .filter(|def| def.tags.contains(&"happy-path"))
            .all(|def| def.moves_value)
    );
}

#[test]
fn every_scenario_name_is_unique_and_the_old_cancel_name_is_gone() {
    crate::link();
    assert!(registry::validate().is_ok());
    assert!(registry::find("cancel_pending").is_none());
}

/// Every interruption lies outside the app, so each scenario that causes
/// one says which: a run that cannot cause it skips the scenario instead
/// of failing on a client that has no way to be interrupted.
#[test]
fn every_interruption_scenario_declares_what_it_needs() {
    for name in tagged("interruption")
        .into_iter()
        .chain(["wallet_unavailable_then_pay"])
    {
        let def = registry::find(name).unwrap();
        assert!(!def.needs.is_empty(), "{name} declares no need");
    }
}

#[test]
fn naming_nothing_selects_every_family_and_never_the_same_scenario_twice() {
    // Arrange
    crate::link();
    let families = [
        "happy-path",
        "cancellation",
        "expiration",
        "dispute",
        "payment-failure",
        "recovery",
    ];

    // Act
    let every: Vec<_> = registry::select(&[], None)
        .unwrap()
        .into_iter()
        .map(|def| def.name)
        .collect();

    // Assert
    let distinct: BTreeSet<_> = every.iter().copied().collect();
    assert_eq!(distinct.len(), every.len(), "{every:?}");
    for family in families {
        assert!(tagged(family).is_subset(&distinct), "{family}");
    }
    assert!(distinct.contains("unauthorized_actions_at_taken"));
}
