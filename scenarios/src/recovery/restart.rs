//! App restart at each durable trade state (Phase 5.1): the app must
//! reconstruct the same order state and must not repeat any action.

use mortsom_android_driver::avd::Actor;
use mortsom_app_contract::OrderState;
use mortsom_scenario_kit::context::{Context, ScenarioError};
use mortsom_scenario_kit::recovery::restart_app;

use super::{NO_VALUE_TIMEOUT_SECS, TRADE_TIMEOUT_SECS};
use crate::flow;

/// Restarts both apps and expects `state` on each.
async fn restart_both(
    ctx: &Context,
    order: &mortsom_app_contract::OrderReference,
    state: OrderState,
) -> Result<(), ScenarioError> {
    for actor in Actor::ALL {
        restart_app(ctx, actor, order, state, ctx.recovery.quiet_window).await?;
    }
    Ok(())
}

/// Pending order survives the maker's restart; nothing is republished.
pub async fn restart_at_pending(ctx: &Context) -> Result<(), ScenarioError> {
    let order = flow::create_pending(ctx).await?;
    restart_app(
        ctx,
        Actor::Alice,
        &order,
        OrderState::Pending,
        ctx.recovery.quiet_window,
    )
    .await?;
    // The order stays open for the cleanup sweep to cancel.
    Ok(())
}

/// Taken (waiting for payment) survives both restarts.
pub async fn restart_at_taken(ctx: &Context) -> Result<(), ScenarioError> {
    let taken = flow::create_and_take(ctx).await?;
    restart_both(ctx, &taken.order, OrderState::Taken).await
}

/// Active (hold invoice paid) survives both restarts; the trade then
/// completes so no escrow is left behind.
pub async fn restart_at_active(ctx: &Context) -> Result<(), ScenarioError> {
    let taken = flow::create_and_take(ctx).await?;
    flow::fund(ctx, &taken.order).await?;
    restart_both(ctx, &taken.order, OrderState::Active).await?;
    flow::fiat_sent(ctx, &taken.order).await?;
    flow::release_and_settle(ctx, &taken).await
}

/// Fiat-sent survives both restarts without a second fiat-sent message.
pub async fn restart_at_fiat_sent(ctx: &Context) -> Result<(), ScenarioError> {
    let taken = flow::create_and_take(ctx).await?;
    flow::fund(ctx, &taken.order).await?;
    flow::fiat_sent(ctx, &taken.order).await?;
    restart_app(
        ctx,
        Actor::Bob,
        &taken.order,
        OrderState::FiatSent,
        ctx.recovery.quiet_window,
    )
    .await?;
    restart_app(
        ctx,
        Actor::Alice,
        &taken.order,
        OrderState::FiatSent,
        ctx.recovery.quiet_window,
    )
    .await?;
    flow::release_and_settle(ctx, &taken).await
}

/// Completed survives both restarts without a second release.
pub async fn restart_at_completed(ctx: &Context) -> Result<(), ScenarioError> {
    let taken = flow::create_and_take(ctx).await?;
    flow::fund(ctx, &taken.order).await?;
    flow::fiat_sent(ctx, &taken.order).await?;
    flow::release_and_settle(ctx, &taken).await?;
    restart_both(ctx, &taken.order, OrderState::Completed).await
}

crate::register_scenario! {
    name: "restart_at_pending",
    body: restart_at_pending,
    tags: &["recovery", "restart"],
    requires: flow::TAKE_REQUIRES,
    timeout_secs: NO_VALUE_TIMEOUT_SECS,
    moves_value: false,
}
crate::register_scenario! {
    name: "restart_at_taken",
    body: restart_at_taken,
    tags: &["recovery", "restart", "parity"],
    requires: flow::TAKE_REQUIRES,
    timeout_secs: NO_VALUE_TIMEOUT_SECS,
    moves_value: false,
}
crate::register_scenario! {
    name: "restart_at_active",
    body: restart_at_active,
    tags: &["recovery", "restart"],
    requires: flow::TRADE_REQUIRES,
    timeout_secs: TRADE_TIMEOUT_SECS,
    moves_value: true,
}
crate::register_scenario! {
    name: "restart_at_fiat_sent",
    body: restart_at_fiat_sent,
    tags: &["recovery", "restart"],
    requires: flow::TRADE_REQUIRES,
    timeout_secs: TRADE_TIMEOUT_SECS,
    moves_value: true,
}
crate::register_scenario! {
    name: "restart_at_completed",
    body: restart_at_completed,
    tags: &["recovery", "restart"],
    requires: flow::TRADE_REQUIRES,
    timeout_secs: TRADE_TIMEOUT_SECS,
    moves_value: true,
}

/// Keeps registrations linked.
pub fn link() {}
