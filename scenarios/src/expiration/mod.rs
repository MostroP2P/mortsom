//! Expiration family (spec §8, tag `expiration`): the daemon's own clocks
//! reaping what nobody touched. Every scenario here waits for a deadline
//! the daemon owns, so each takes as long as the stack's limits say; the
//! proof is the daemon's published state after a baseline, never a screen,
//! and the escrow coming back where one was held.

use mortsom_app_contract::{Capability, OrderKind, OrderState};
use mortsom_common::Actor;
use mortsom_scenario_kit::actions;
use mortsom_scenario_kit::context::{Context, ScenarioError};

use crate::flow;

/// Deadline for a scenario that waits out one daemon clock.
pub const CLOCK_TIMEOUT_SECS: u64 = 45 * 60;

/// Capabilities needed to fund a taken buy order and then wait.
const FUNDED_TAKE_REQUIRES: &[Capability] = &[
    Capability::Onboarding,
    Capability::CustomNodeSelection,
    Capability::LocalRelayConfiguration,
    Capability::Nwc,
    Capability::CreateOrder,
    Capability::TakeOrder,
    Capability::HoldInvoicePayment,
    Capability::BuyerInvoice,
];

/// Capabilities needed to create an order that asks for the stack's short
/// lifetime and then to close it.
const EXPIRY_REQUIRES: &[Capability] = &[
    Capability::Onboarding,
    Capability::CustomNodeSelection,
    Capability::LocalRelayConfiguration,
    Capability::Nwc,
    Capability::CreateOrder,
    Capability::TakeOrder,
    Capability::BuyerInvoice,
    Capability::Cancel,
    Capability::OrderExpiry,
];

/// What the book shows for an untaken order the daemon expired: its own
/// status is `expired`, but the public event carries `canceled` -- the
/// daemon maps every closed-without-a-trade status onto one wire word.
const EXPIRED_ON_THE_BOOK: &[OrderState] = &[OrderState::Expired, OrderState::Cancelled];

/// Nobody takes the order and it lapses. The build asks the daemon for
/// the lifetime the stack declares (`order_expiration_secs`), since the
/// daemon's own default is hour-granular; after it the daemon closes the
/// order on the book, and the maker's app stops holding it open.
pub async fn pending_order_expires(ctx: &Context) -> Result<(), ScenarioError> {
    let order = flow::create_pending(ctx).await?;
    let since = actions::relay_cursor(ctx);
    actions::wait_relay_state_since(
        ctx,
        &order,
        EXPIRED_ON_THE_BOOK,
        since,
        ctx.expiration_timeout,
    )
    .await?;
    actions::expect_closed(ctx, Actor::Alice, &order, ctx.expiration_timeout).await?;
    actions::mark_resolved(ctx, &order);
    Ok(())
}

/// The seller never pays the hold invoice: after its taker window the
/// daemon cancels the taken sell order (never "expired", which is reserved
/// for orders nobody took) and both apps stop holding it open.
pub async fn hold_invoice_expires(ctx: &Context) -> Result<(), ScenarioError> {
    let taken = flow::create_and_take(ctx).await?;
    let since = actions::relay_cursor(ctx);
    actions::wait_relay_state_since(
        ctx,
        &taken.order,
        &[OrderState::Cancelled],
        since,
        ctx.expiration_timeout,
    )
    .await?;
    for actor in Actor::ALL {
        actions::expect_closed(ctx, actor, &taken.order, ctx.expiration_timeout).await?;
    }
    actions::mark_resolved(ctx, &taken.order);
    Ok(())
}

/// The buyer never provides a payout invoice in time: Bob's buy order is
/// taken and funded by Alice, Bob never invoices, and after its window the
/// daemon cancels the order and returns Alice's escrow.
pub async fn buyer_invoice_deadline(ctx: &Context) -> Result<(), ScenarioError> {
    let order = flow::create_pending_with(
        ctx,
        Actor::Bob,
        &mortsom_app_contract::OrderDraft {
            kind: OrderKind::Buy,
            ..flow::sell_draft(ctx)
        },
    )
    .await?;
    actions::take_order(ctx, Actor::Alice, &order).await?;
    actions::wait_both(ctx, &order, OrderState::Taken).await?;
    let since = actions::relay_cursor(ctx);
    actions::pay_hold_invoice(ctx, Actor::Alice, &order).await?;
    actions::wait_relay_state_since(
        ctx,
        &order,
        &[OrderState::Cancelled],
        since,
        ctx.expiration_timeout,
    )
    .await?;
    for actor in Actor::ALL {
        actions::expect_closed(ctx, actor, &order, ctx.expiration_timeout).await?;
    }
    actions::expect_hold_refunded(ctx, &order).await?;
    actions::mark_resolved(ctx, &order);
    Ok(())
}

crate::register_scenario! {
    name: "pending_order_expires",
    body: pending_order_expires,
    tags: &["expiration"],
    requires: EXPIRY_REQUIRES,
    timeout_secs: CLOCK_TIMEOUT_SECS,
    moves_value: false,
}
crate::register_scenario! {
    name: "hold_invoice_expires",
    body: hold_invoice_expires,
    tags: &["expiration"],
    requires: flow::TAKE_REQUIRES,
    timeout_secs: CLOCK_TIMEOUT_SECS,
    moves_value: false,
}
crate::register_scenario! {
    name: "buyer_invoice_deadline",
    body: buyer_invoice_deadline,
    tags: &["expiration"],
    requires: FUNDED_TAKE_REQUIRES,
    timeout_secs: CLOCK_TIMEOUT_SECS,
    moves_value: true,
}

/// Keeps registrations linked.
pub fn link() {}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
