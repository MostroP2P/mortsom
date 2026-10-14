//! Interruptions around a taken order (Phase 5.2): backgrounding, force
//! stop, connectivity loss, relay restart, duplicate and stale daemon
//! events, rotation. None of these move value.

use mortsom_app_contract::OrderState;
use mortsom_common::Actor;
use mortsom_scenario_kit::context::{Context, ScenarioError};
use mortsom_scenario_kit::recovery::{
    Replay, background_app, network_outage, replay_order_events, restart_relay, rotate_device,
};
use mortsom_scenario_kit::registry::Need;

use super::NO_VALUE_TIMEOUT_SECS;
use crate::flow;

/// Both apps go to the background and come back to the same taken order.
pub async fn background_at_taken(ctx: &Context) -> Result<(), ScenarioError> {
    let taken = flow::create_and_take(ctx).await?;
    for actor in Actor::ALL {
        background_app(
            ctx,
            actor,
            ctx.recovery.background_secs,
            &taken.order,
            OrderState::Taken,
            ctx.recovery.quiet_window,
        )
        .await?;
    }
    Ok(())
}

/// The buyer loses connectivity after taking; the taken order is still
/// there once the network returns.
pub async fn network_loss_at_taken(ctx: &Context) -> Result<(), ScenarioError> {
    let taken = flow::create_and_take(ctx).await?;
    network_outage(
        ctx,
        Actor::Bob,
        ctx.recovery.outage,
        &taken.order,
        OrderState::Taken,
    )
    .await?;
    network_outage(
        ctx,
        Actor::Alice,
        ctx.recovery.outage,
        &taken.order,
        OrderState::Taken,
    )
    .await?;
    Ok(())
}

/// The run relay restarts under a taken order; both apps reconnect and
/// keep showing it.
pub async fn relay_restart_at_taken(ctx: &Context) -> Result<(), ScenarioError> {
    let taken = flow::create_and_take(ctx).await?;
    restart_relay(ctx, &taken.order, OrderState::Taken).await
}

/// Every daemon event already seen for the order is delivered again; the
/// apps must not move.
pub async fn duplicate_events_at_taken(ctx: &Context) -> Result<(), ScenarioError> {
    let taken = flow::create_and_take(ctx).await?;
    replay_order_events(ctx, &taken.order, Replay::Duplicates, OrderState::Taken).await?;
    Ok(())
}

/// The original pending event arrives late while the order is taken; the
/// apps must keep the newer state.
pub async fn stale_event_at_taken(ctx: &Context) -> Result<(), ScenarioError> {
    let taken = flow::create_and_take(ctx).await?;
    replay_order_events(ctx, &taken.order, Replay::Stale, OrderState::Taken).await?;
    Ok(())
}

/// Rotation on both devices keeps the taken order on screen.
pub async fn rotation_at_taken(ctx: &Context) -> Result<(), ScenarioError> {
    let taken = flow::create_and_take(ctx).await?;
    for actor in Actor::ALL {
        rotate_device(ctx, actor, &taken.order, OrderState::Taken).await?;
    }
    Ok(())
}

crate::register_scenario! {
    name: "background_at_taken",
    body: background_at_taken,
    tags: &["recovery", "interruption"],
    requires: flow::TAKE_REQUIRES,
    timeout_secs: NO_VALUE_TIMEOUT_SECS,
    moves_value: false,
    needs: &[Need::Background],
}
crate::register_scenario! {
    name: "network_loss_at_taken",
    body: network_loss_at_taken,
    tags: &["recovery", "interruption"],
    requires: flow::TAKE_REQUIRES,
    timeout_secs: NO_VALUE_TIMEOUT_SECS,
    moves_value: false,
    needs: &[Need::Connectivity],
}
crate::register_scenario! {
    name: "relay_restart_at_taken",
    body: relay_restart_at_taken,
    tags: &["recovery", "interruption", "relay"],
    requires: flow::TAKE_REQUIRES,
    timeout_secs: NO_VALUE_TIMEOUT_SECS,
    moves_value: false,
    needs: &[Need::RelayFaults],
}
crate::register_scenario! {
    name: "duplicate_events_at_taken",
    body: duplicate_events_at_taken,
    tags: &["recovery", "interruption", "relay"],
    requires: flow::TAKE_REQUIRES,
    timeout_secs: NO_VALUE_TIMEOUT_SECS,
    moves_value: false,
    needs: &[Need::RelayFaults],
}
crate::register_scenario! {
    name: "stale_event_at_taken",
    body: stale_event_at_taken,
    tags: &["recovery", "interruption", "relay"],
    requires: flow::TAKE_REQUIRES,
    timeout_secs: NO_VALUE_TIMEOUT_SECS,
    moves_value: false,
    needs: &[Need::RelayFaults],
}
crate::register_scenario! {
    name: "rotation_at_taken",
    body: rotation_at_taken,
    tags: &["recovery", "interruption"],
    requires: flow::TAKE_REQUIRES,
    timeout_secs: NO_VALUE_TIMEOUT_SECS,
    moves_value: false,
    needs: &[Need::Rotation],
}

/// Keeps registrations linked.
pub fn link() {}
