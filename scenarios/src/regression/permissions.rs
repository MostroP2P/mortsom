//! Unauthorized-action UX (Phase 5.4): each side sees only the actions its
//! role and the order state allow, and nothing illegal reaches the relay.

use mortsom_android_driver::avd::Actor;
use mortsom_scenario_kit::cancel::expect_actions;
use mortsom_scenario_kit::context::{Context, ScenarioError};
use mortsom_scenario_kit::recovery::{expect_no_actor_message, message_baseline};

use crate::flow;
use crate::recovery::NO_VALUE_TIMEOUT_SECS;

/// While the seller still owes the hold invoice: the seller may pay or
/// cancel but not release nor report fiat; the buyer may cancel but neither
/// report fiat, release nor pay.
pub async fn unauthorized_actions_at_taken(ctx: &Context) -> Result<(), ScenarioError> {
    let taken = flow::create_and_take(ctx).await?;
    let baseline = message_baseline(ctx);
    expect_actions(
        ctx,
        Actor::Alice,
        &taken.order,
        &["pay-invoice"],
        &["release", "fiat-sent", "add-invoice", "rate"],
    )
    .await?;
    expect_actions(
        ctx,
        Actor::Bob,
        &taken.order,
        &[],
        &["release", "fiat-sent", "pay-invoice", "rate"],
    )
    .await?;
    expect_no_actor_message(
        ctx,
        baseline,
        ctx.recovery.quiet_window,
        "inspecting trade actions",
    )
    .await
}

crate::register_scenario! {
    name: "unauthorized_actions_at_taken",
    body: unauthorized_actions_at_taken,
    tags: &["regression", "permissions", "parity"],
    requires: flow::TAKE_REQUIRES,
    timeout_secs: NO_VALUE_TIMEOUT_SECS,
    moves_value: false,
}

/// Keeps registrations linked.
pub fn link() {}
