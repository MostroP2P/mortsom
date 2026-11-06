//! Unauthorized-action UX (Phase 5.4): each side sees only the actions its
//! role and the order state allow, and nothing illegal reaches the relay.

use mortsom_app_contract::Capability;
use mortsom_common::Actor;
use mortsom_scenario_kit::cancel::expect_actions;
use mortsom_scenario_kit::context::{Context, ScenarioError};
use mortsom_scenario_kit::recovery::{expect_no_actor_message, message_baseline};

use crate::flow;
use crate::recovery::NO_VALUE_TIMEOUT_SECS;

/// Capabilities needed to reach a taken order and read which trade actions
/// each side is offered.
const OFFERED_ACTIONS_REQUIRES: &[Capability] = &[
    Capability::Onboarding,
    Capability::CustomNodeSelection,
    Capability::LocalRelayConfiguration,
    Capability::Nwc,
    Capability::CreateOrder,
    Capability::TakeOrder,
    Capability::BuyerInvoice,
    Capability::OfferedActions,
];

/// While the seller still owes the hold invoice: the seller may pay or
/// cancel but not release nor report fiat; the buyer may cancel but neither
/// report fiat, release nor pay.
pub async fn unauthorized_actions_at_taken(ctx: &Context) -> Result<(), ScenarioError> {
    let taken = flow::create_and_take(ctx).await?;
    let baseline = message_baseline(ctx);
    // Cancelling is allowed for both sides while the invoice is unpaid, and
    // the scenario says so: requiring it catches a build that hides the way
    // out of a trade nobody has funded yet.
    expect_actions(
        ctx,
        Actor::Alice,
        &taken.order,
        &["pay-invoice", "cancel"],
        &["release", "fiat-sent", "add-invoice", "rate"],
    )
    .await?;
    expect_actions(
        ctx,
        Actor::Bob,
        &taken.order,
        &["cancel"],
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
    requires: OFFERED_ACTIONS_REQUIRES,
    timeout_secs: NO_VALUE_TIMEOUT_SECS,
    moves_value: false,
}

/// Keeps registrations linked.
pub fn link() {}
