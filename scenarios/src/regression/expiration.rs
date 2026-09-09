//! Expiration family: the daemon's own clocks reaping what nobody touched.

use mortsom_android_driver::avd::Actor;
use mortsom_app_contract::OrderState;
use mortsom_scenario_kit::actions;
use mortsom_scenario_kit::context::{Context, ScenarioError};

use crate::flow;

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
    name: "expiration_unpaid",
    body: expiration_unpaid,
    tags: &["regression", "expiration"],
    requires: flow::TAKE_REQUIRES,
    timeout_secs: 45 * 60,
    moves_value: false,
}

/// Keeps registrations linked.
pub fn link() {}
