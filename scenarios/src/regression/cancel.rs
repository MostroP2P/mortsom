//! Cancellation and expiration (Phase 5.4).

use mortsom_android_driver::avd::Actor;
use mortsom_app_contract::{CancelOutcome, OrderState};
use mortsom_scenario_kit::actions;
use mortsom_scenario_kit::cancel::cancel_order;
use mortsom_scenario_kit::context::{Context, ScenarioError};

use crate::flow;
use crate::recovery::{NO_VALUE_TIMEOUT_SECS, TRADE_TIMEOUT_SECS};

/// The maker cancels a pending order; both apps and the relay agree.
pub async fn cancel_pending(ctx: &Context) -> Result<(), ScenarioError> {
    let order = flow::create_pending(ctx).await?;
    let outcome = cancel_order(ctx, Actor::Alice, &order).await?;
    if outcome != CancelOutcome::Cancelled {
        return Err(ScenarioError::product(format!(
            "pending order {} could not be cancelled by its maker: {outcome:?}",
            order.id
        )));
    }
    flow::expect_cancelled(ctx, &order).await
}

/// The taker cancels before the seller pays: the daemon republishes the
/// order as pending for the maker.
pub async fn taker_cancel_at_taken(ctx: &Context) -> Result<(), ScenarioError> {
    let taken = flow::create_and_take(ctx).await?;
    let outcome = cancel_order(ctx, Actor::Bob, &taken.order).await?;
    if outcome != CancelOutcome::Cancelled {
        return Err(ScenarioError::product(format!(
            "taker could not cancel taken order {}: {outcome:?}",
            taken.order.id
        )));
    }
    actions::wait_app_state(ctx, Actor::Alice, &taken.order, OrderState::Pending).await?;
    actions::wait_relay_state(ctx, &taken.order, OrderState::Pending).await?;
    // The republished order stays open for the cleanup sweep.
    Ok(())
}

/// Cooperative cancellation of a funded order: the seller asks, the buyer
/// agrees, the escrow returns and both apps show the cancellation.
pub async fn cooperative_cancel(ctx: &Context) -> Result<(), ScenarioError> {
    let taken = flow::create_and_take(ctx).await?;
    flow::fund(ctx, &taken.order).await?;
    let first = cancel_order(ctx, Actor::Alice, &taken.order).await?;
    if first != CancelOutcome::Cancelled {
        return Err(ScenarioError::product(format!(
            "seller could not start a cooperative cancel of {}: {first:?}",
            taken.order.id
        )));
    }
    // One side asking must not cancel the trade on its own: the buyer keeps
    // seeing it active for the whole quiet window.
    actions::expect_state_holds(
        ctx,
        Actor::Bob,
        &taken.order,
        OrderState::Active,
        ctx.recovery.quiet_window,
    )
    .await?;
    let second = cancel_order(ctx, Actor::Bob, &taken.order).await?;
    if second != CancelOutcome::Cancelled {
        return Err(ScenarioError::product(format!(
            "buyer could not accept the cooperative cancel of {}: {second:?}",
            taken.order.id
        )));
    }
    flow::expect_cancelled(ctx, &taken.order).await
}

/// The seller never pays: the daemon cancels the taken sell order after its
/// `expiration_seconds` and both apps show it.
pub async fn expiration_unpaid(ctx: &Context) -> Result<(), ScenarioError> {
    let taken = flow::create_and_take(ctx).await?;
    // The daemon cancels an unpaid taken sell order (never "expired", which
    // is reserved for orders nobody took).
    for actor in Actor::ALL {
        actions::wait_app_state_in(
            ctx,
            actor,
            &taken.order,
            &[OrderState::Cancelled],
            ctx.expiration_timeout,
        )
        .await?;
    }
    actions::wait_relay_state(ctx, &taken.order, OrderState::Cancelled).await?;
    actions::mark_resolved(ctx, &taken.order);
    Ok(())
}

crate::register_scenario! {
    name: "cancel_pending",
    body: cancel_pending,
    tags: &["regression", "cancel"],
    requires: flow::CANCEL_REQUIRES,
    timeout_secs: NO_VALUE_TIMEOUT_SECS,
    moves_value: false,
}
crate::register_scenario! {
    name: "taker_cancel_at_taken",
    body: taker_cancel_at_taken,
    tags: &["regression", "cancel"],
    requires: flow::CANCEL_REQUIRES,
    timeout_secs: NO_VALUE_TIMEOUT_SECS,
    moves_value: false,
}
crate::register_scenario! {
    name: "cooperative_cancel",
    body: cooperative_cancel,
    tags: &["regression", "cancel"],
    requires: flow::TRADE_REQUIRES,
    timeout_secs: TRADE_TIMEOUT_SECS,
    moves_value: true,
}
crate::register_scenario! {
    name: "expiration_unpaid",
    body: expiration_unpaid,
    tags: &["regression", "expiration"],
    requires: flow::TAKE_REQUIRES,
    timeout_secs: 45 * 60,
    moves_value: false,
}

/// Keeps registrations linked.
pub fn link() {}
