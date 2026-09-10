//! Cancellation family (spec §8, tag `cancellation`): orders that end
//! before the sats move, and *who* may end them depends on how far they
//! got. A pending order is the maker's to close alone; a taken one comes
//! back to the book when the taker walks away; a funded one needs both.

use mortsom_android_driver::avd::Actor;
use mortsom_app_contract::{CancelOutcome, Capability, OrderState};
use mortsom_scenario_kit::actions;
use mortsom_scenario_kit::cancel::cancel_order;
use mortsom_scenario_kit::context::{Context, ScenarioError};

use crate::flow;
use crate::recovery::{NO_VALUE_TIMEOUT_SECS, TRADE_TIMEOUT_SECS};

/// Capabilities needed to cancel a funded order: the trade up to funding,
/// and the cancel control on both sides.
pub const FUNDED_CANCEL_REQUIRES: &[Capability] = &[
    Capability::Onboarding,
    Capability::CustomNodeSelection,
    Capability::LocalRelayConfiguration,
    Capability::Nwc,
    Capability::CreateOrder,
    Capability::TakeOrder,
    Capability::HoldInvoicePayment,
    Capability::BuyerInvoice,
    Capability::Cancel,
];

/// What the daemon may publish once it reaps an abandoned take.
const REAPED: &[OrderState] = &[
    OrderState::Pending,
    OrderState::Cancelled,
    OrderState::Expired,
];

/// The maker cancels while the order is still pending. Nobody else is
/// involved, so it is immediate and unilateral; both apps and the relay
/// agree.
pub async fn cancel_before_taken(ctx: &Context) -> Result<(), ScenarioError> {
    let order = flow::create_pending(ctx).await?;
    let outcome = cancel_order(ctx, Actor::Alice, &order).await?;
    if outcome != CancelOutcome::Cancelled {
        return Err(ScenarioError::product(format!(
            "pending order {} could not be cancelled by its maker: {outcome:?}",
            order.id
        )));
    }
    flow::expect_cancelled(ctx, &order, &[Actor::Alice]).await
}

/// The taker takes the order and abandons it without an invoice. The
/// daemon must reap the take after its window and put the order back on
/// the book, or close it; the maker's app follows the book.
pub async fn cancel_after_take_unpaid(ctx: &Context) -> Result<(), ScenarioError> {
    let order = flow::create_pending(ctx).await?;
    // The original `pending` must not satisfy the wait for the reap.
    let since = actions::relay_cursor(ctx);
    actions::take_order(ctx, Actor::Bob, &order).await?;
    actions::wait_app_state(ctx, Actor::Bob, &order, OrderState::Taken).await?;
    actions::wait_relay_state(ctx, &order, OrderState::Taken).await?;
    let reaped =
        actions::wait_relay_state_since(ctx, &order, REAPED, since, ctx.expiration_timeout).await?;
    if reaped == OrderState::Pending {
        // Back on the book for the maker: the cleanup sweep closes it.
        actions::wait_app_state(ctx, Actor::Alice, &order, OrderState::Pending).await?;
        return Ok(());
    }
    // Closed by the daemon: the maker's app drops a trade that never went
    // active, or shows it closed; either way it no longer holds it open.
    actions::expect_closed(ctx, Actor::Alice, &order, ctx.expiration_timeout).await?;
    actions::mark_resolved(ctx, &order);
    Ok(())
}

/// The taker cancels before the seller pays: the daemon republishes the
/// order as pending for the maker.
pub async fn taker_cancel_at_taken(ctx: &Context) -> Result<(), ScenarioError> {
    let taken = flow::create_and_take(ctx).await?;
    let since = actions::relay_cursor(ctx);
    let outcome = cancel_order(ctx, Actor::Bob, &taken.order).await?;
    if outcome != CancelOutcome::Cancelled {
        return Err(ScenarioError::product(format!(
            "taker could not cancel taken order {}: {outcome:?}",
            taken.order.id
        )));
    }
    actions::wait_app_state(ctx, Actor::Alice, &taken.order, OrderState::Pending).await?;
    actions::wait_relay_state_since(
        ctx,
        &taken.order,
        &[OrderState::Pending],
        since,
        ctx.state_timeout,
    )
    .await?;
    // The republished order stays open for the cleanup sweep.
    Ok(())
}

/// Both parties agree to undo a funded order: the seller asks, the buyer
/// confirms, the escrow returns and both apps show the cancellation.
pub async fn cooperative_cancel(ctx: &Context) -> Result<(), ScenarioError> {
    let taken = flow::create_take_and_fund(ctx).await?;
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
    flow::expect_cancelled(ctx, &taken.order, &Actor::ALL).await
}

/// One party alone tries to cancel a funded order. The daemon must not
/// cancel it: a lone cancel only opens the cooperative flow, so the buyer
/// keeps seeing the trade active and the book never shows it cancelled.
/// Asserts an absence, so it spends its window proving nothing happened;
/// the buyer then accepts, which returns the escrow and is not the
/// assertion.
pub async fn unilateral_cancel_rejected(ctx: &Context) -> Result<(), ScenarioError> {
    let taken = flow::create_take_and_fund(ctx).await?;
    let since = actions::relay_cursor(ctx);
    let outcome = cancel_order(ctx, Actor::Alice, &taken.order).await?;
    if outcome != CancelOutcome::Cancelled {
        return Err(ScenarioError::product(format!(
            "seller could not ask to cancel funded order {}: {outcome:?}",
            taken.order.id
        )));
    }
    actions::expect_state_holds(
        ctx,
        Actor::Bob,
        &taken.order,
        OrderState::Active,
        ctx.recovery.quiet_window,
    )
    .await?;
    actions::expect_relay_state_absent(
        ctx,
        &taken.order,
        OrderState::Cancelled,
        since,
        ctx.recovery.quiet_window,
    )
    .await?;
    let accepted = cancel_order(ctx, Actor::Bob, &taken.order).await?;
    if accepted != CancelOutcome::Cancelled {
        return Err(ScenarioError::product(format!(
            "buyer could not accept the cooperative cancel of {}: {accepted:?}",
            taken.order.id
        )));
    }
    flow::expect_cancelled(ctx, &taken.order, &Actor::ALL).await
}

crate::register_scenario! {
    name: "cancel_before_taken",
    body: cancel_before_taken,
    tags: &["smoke", "cancellation", "parity"],
    requires: flow::CANCEL_REQUIRES,
    timeout_secs: NO_VALUE_TIMEOUT_SECS,
    moves_value: false,
}
crate::register_scenario! {
    name: "cancel_after_take_unpaid",
    body: cancel_after_take_unpaid,
    tags: &["cancellation"],
    requires: flow::TAKE_REQUIRES,
    timeout_secs: 45 * 60,
    moves_value: false,
}
crate::register_scenario! {
    name: "taker_cancel_at_taken",
    body: taker_cancel_at_taken,
    tags: &["cancellation"],
    requires: flow::CANCEL_REQUIRES,
    timeout_secs: NO_VALUE_TIMEOUT_SECS,
    moves_value: false,
}
crate::register_scenario! {
    name: "cooperative_cancel",
    body: cooperative_cancel,
    tags: &["cancellation", "parity"],
    requires: FUNDED_CANCEL_REQUIRES,
    timeout_secs: TRADE_TIMEOUT_SECS,
    moves_value: true,
}
crate::register_scenario! {
    name: "unilateral_cancel_rejected",
    body: unilateral_cancel_rejected,
    tags: &["cancellation"],
    requires: FUNDED_CANCEL_REQUIRES,
    timeout_secs: TRADE_TIMEOUT_SECS,
    moves_value: true,
}

/// Keeps registrations linked.
pub fn link() {}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
