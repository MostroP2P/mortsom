//! Dispute family (spec §8, tag `dispute`): a funded trade one party
//! escalates, and the three ways it ends -- a solver settles it for the
//! buyer, a solver cancels it for the seller, or the seller releases and
//! the dispute closes on its own. The solver is the run's, registered
//! with the daemon by the coordinator; the proof of every step is the
//! daemon's published dispute event and, at the end, where the escrow
//! went.

use mortsom_app_contract::{Capability, OrderReference, OrderState};
use mortsom_common::Actor;
use mortsom_scenario_kit::actions::{self, Dispute, dispute_status};
use mortsom_scenario_kit::context::{Context, ScenarioError};

use crate::flow;
use crate::recovery::TRADE_TIMEOUT_SECS;

/// Capabilities needed to fund a trade and dispute it.
pub const DISPUTE_REQUIRES: &[Capability] = &[
    Capability::Onboarding,
    Capability::CustomNodeSelection,
    Capability::LocalRelayConfiguration,
    Capability::Nwc,
    Capability::CreateOrder,
    Capability::TakeOrder,
    Capability::HoldInvoicePayment,
    Capability::BuyerInvoice,
    Capability::FiatSent,
    Capability::Release,
    Capability::CompletedObservation,
    Capability::Dispute,
];

/// `actor` opens a dispute on `order`; the daemon publishes it as
/// initiated by that side, and both apps show the order disputed.
async fn open_and_confirm(
    ctx: &Context,
    actor: Actor,
    order: &OrderReference,
) -> Result<Dispute, ScenarioError> {
    let since = actions::relay_cursor(ctx);
    actions::open_dispute(ctx, actor, order).await?;
    let dispute = actions::wait_dispute_opened(ctx, since).await?;
    let expected = match actor {
        Actor::Alice => "seller",
        Actor::Bob => "buyer",
    };
    if dispute.initiator != expected {
        return Err(ScenarioError::product(format!(
            "the daemon published dispute {} as opened by the {}, but the {expected} opened it",
            dispute.id, dispute.initiator
        )));
    }
    actions::wait_both(ctx, order, OrderState::Disputed).await?;
    Ok(dispute)
}

/// The solver takes `dispute`; the daemon publishes it in progress.
async fn solver_takes(
    ctx: &Context,
    dispute: &Dispute,
    order: &OrderReference,
) -> Result<(), ScenarioError> {
    let since = actions::relay_cursor(ctx);
    actions::solver_takes_dispute(ctx, dispute, order).await?;
    actions::wait_dispute_status(ctx, &dispute.id, dispute_status::IN_PROGRESS, since).await?;
    Ok(())
}

/// The buyer paid and the seller will not release: Bob disputes at
/// fiat-sent, the solver takes the dispute and settles it for him. The
/// escrow settles on Alice's node and the payout lands on Bob's.
pub async fn dispute_settled_by_solver(ctx: &Context) -> Result<(), ScenarioError> {
    let taken = flow::create_take_and_fund(ctx).await?;
    flow::fiat_sent(ctx, &taken.order).await?;
    let dispute = open_and_confirm(ctx, Actor::Bob, &taken.order).await?;
    solver_takes(ctx, &dispute, &taken.order).await?;
    let hash = taken.payout_hash.as_deref().ok_or_else(|| {
        ScenarioError::wallet("buyer payout hash is required before the solver settles")
    })?;
    let since = actions::relay_cursor(ctx);
    actions::solver_settles(ctx, &taken.order).await?;
    actions::wait_dispute_status(ctx, &dispute.id, dispute_status::SETTLED, since).await?;
    actions::wait_both(ctx, &taken.order, OrderState::Completed).await?;
    actions::wait_relay_state_since(
        ctx,
        &taken.order,
        &[OrderState::Completed],
        since,
        ctx.state_timeout,
    )
    .await?;
    actions::settle_hold_payment(ctx, &taken.order).await?;
    actions::expect_payout_settled(ctx, Actor::Bob, &taken.order, hash).await?;
    actions::mark_resolved(ctx, &taken.order);
    Ok(())
}

/// The buyer never paid: Alice disputes the active order, the solver
/// takes the dispute and cancels the order for her. The escrow comes back.
pub async fn dispute_canceled_by_solver(ctx: &Context) -> Result<(), ScenarioError> {
    let taken = flow::create_take_and_fund(ctx).await?;
    let dispute = open_and_confirm(ctx, Actor::Alice, &taken.order).await?;
    solver_takes(ctx, &dispute, &taken.order).await?;
    let since = actions::relay_cursor(ctx);
    actions::solver_cancels(ctx, &taken.order).await?;
    actions::wait_dispute_status(ctx, &dispute.id, dispute_status::SELLER_REFUNDED, since).await?;
    actions::wait_both(ctx, &taken.order, OrderState::Cancelled).await?;
    actions::wait_relay_state_since(
        ctx,
        &taken.order,
        &[OrderState::Cancelled],
        since,
        ctx.state_timeout,
    )
    .await?;
    actions::expect_hold_refunded(ctx, &taken.order).await?;
    actions::mark_resolved(ctx, &taken.order);
    Ok(())
}

/// The parties sort it out themselves: Bob disputes at fiat-sent, then
/// Alice releases anyway. The trade completes as any other and the daemon
/// closes the dispute as released without a solver ever taking it.
pub async fn dispute_released_by_seller(ctx: &Context) -> Result<(), ScenarioError> {
    let taken = flow::create_take_and_fund(ctx).await?;
    flow::fiat_sent(ctx, &taken.order).await?;
    let dispute = open_and_confirm(ctx, Actor::Bob, &taken.order).await?;
    let since = actions::relay_cursor(ctx);
    flow::release_and_settle(ctx, &taken).await?;
    actions::wait_dispute_status(ctx, &dispute.id, dispute_status::RELEASED, since).await?;
    Ok(())
}

crate::register_scenario! {
    name: "dispute_settled_by_solver",
    body: dispute_settled_by_solver,
    tags: &["dispute"],
    requires: DISPUTE_REQUIRES,
    timeout_secs: TRADE_TIMEOUT_SECS,
    moves_value: true,
    needs_app_wallet: false,
    needs_solver: true,
}
crate::register_scenario! {
    name: "dispute_canceled_by_solver",
    body: dispute_canceled_by_solver,
    tags: &["dispute"],
    requires: DISPUTE_REQUIRES,
    timeout_secs: TRADE_TIMEOUT_SECS,
    moves_value: true,
    needs_app_wallet: false,
    needs_solver: true,
}
crate::register_scenario! {
    name: "dispute_released_by_seller",
    body: dispute_released_by_seller,
    tags: &["dispute"],
    requires: DISPUTE_REQUIRES,
    timeout_secs: TRADE_TIMEOUT_SECS,
    moves_value: true,
}

/// Keeps registrations linked.
pub fn link() {}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
