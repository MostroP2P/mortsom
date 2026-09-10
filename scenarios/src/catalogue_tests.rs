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
