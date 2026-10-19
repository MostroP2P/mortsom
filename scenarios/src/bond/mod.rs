//! Anti-abuse bond family (spec §8.8, tag `bond`): the bonds a bonded daemon
//! asks both sides for, and every way they end. A maker's bond gates the
//! order's publication; a taker's bond gates the take, which the book
//! learns of only once the bond locks. On every honest exit the daemon
//! cancels the HTLC it held, so the bond comes back to its payer's node.
//! A bond is slashed only by a solver's directive or by a waiting-state
//! timeout (`slash`); the winner is then offered the share the node does
//! not keep, claims it with an invoice from their node, and is paid. Every
//! bond here is paid from a node Mortsom holds, from outside the app. The
//! stack must be built with `stack up --bond`: the run skips this family on
//! any other, and a scenario reached without a declared bond ends as
//! environment misconfiguration before an order exists.

use mortsom_app_contract::{Capability, OrderKind, OrderReference, OrderState};
use mortsom_common::Actor;
use mortsom_scenario_kit::actions;
use mortsom_scenario_kit::cancel::cancel_order;
use mortsom_scenario_kit::context::{BondSlash, Context, ScenarioError};

use crate::flow;
use crate::recovery::TRADE_TIMEOUT_SECS;

mod slash;

/// Capabilities needed to complete a bonded trade end to end.
pub const BOND_TRADE_REQUIRES: &[Capability] = &[
    Capability::Onboarding,
    Capability::CustomNodeSelection,
    Capability::LocalRelayConfiguration,
    Capability::Nwc,
    Capability::CreateOrder,
    Capability::TakeOrder,
    Capability::Bond,
    Capability::HoldInvoicePayment,
    Capability::BuyerInvoice,
    Capability::FiatSent,
    Capability::Release,
    Capability::CompletedObservation,
];

/// Capabilities needed to dispute a bonded trade.
pub const BOND_DISPUTE_REQUIRES: &[Capability] = &[
    Capability::Onboarding,
    Capability::CustomNodeSelection,
    Capability::LocalRelayConfiguration,
    Capability::Nwc,
    Capability::CreateOrder,
    Capability::TakeOrder,
    Capability::Bond,
    Capability::HoldInvoicePayment,
    Capability::BuyerInvoice,
    Capability::FiatSent,
    Capability::Dispute,
    Capability::Cancel,
];

/// Capabilities needed to create or take an order and walk away from its
/// bond, or let it be slashed.
pub const BOND_TAKE_REQUIRES: &[Capability] = &[
    Capability::Onboarding,
    Capability::CustomNodeSelection,
    Capability::LocalRelayConfiguration,
    Capability::Nwc,
    Capability::CreateOrder,
    Capability::TakeOrder,
    Capability::Bond,
    Capability::Cancel,
];

/// Capabilities needed to take an order whose escrow then waits on the
/// seller's node: a bonded take on a buy, with the escrow paid or not.
pub const BOND_ESCROW_REQUIRES: &[Capability] = &[
    Capability::Onboarding,
    Capability::CustomNodeSelection,
    Capability::LocalRelayConfiguration,
    Capability::Nwc,
    Capability::CreateOrder,
    Capability::TakeOrder,
    Capability::Bond,
    Capability::HoldInvoicePayment,
    Capability::BuyerInvoice,
    Capability::Cancel,
];

/// Deadline for the scenarios that wait for the daemon's own clock.
pub const SLASH_TIMEOUT_SECS: u64 = 40 * 60;

/// The labels an app shows while its user's bond is outstanding.
const WAITING_BOND: &[&str] = &["waiting-bond", "waiting-taker-bond", "waiting-maker-bond"];
/// The labels the buyer's app shows while the daemon waits for an invoice.
const WAITING_INVOICE: &[&str] = &["waiting-invoice", "waiting-buyer-invoice"];
/// The labels the seller's app shows while the daemon waits for the escrow.
const WAITING_PAYMENT: &[&str] = &["waiting-payment", "waiting-seller-to-pay"];

/// The environment error a maker-bond scenario ends with on a stack whose
/// policy bonds no maker.
fn require_maker_bond(ctx: &Context) -> Result<(), ScenarioError> {
    if actions::bond_policy(ctx)?.maker_posts() {
        return Ok(());
    }
    Err(ScenarioError::environment(
        "the stack's bond policy bonds no maker; build it with `apply_to = \"both\"`",
    ))
}

/// `maker` creates a fixed-price order of `kind`. Under a policy that
/// bonds makers the app parks it on the maker's bond and the daemon
/// publishes nothing until it locks; the bond is paid from the maker's
/// node and the order then reaches the maker's app and the book pending.
/// With `prove_unpublished` the book is watched for the whole quiet
/// window before the bond is paid.
async fn create_bonded(
    ctx: &Context,
    maker: Actor,
    kind: OrderKind,
    prove_unpublished: bool,
) -> Result<OrderReference, ScenarioError> {
    let policy = actions::bond_policy(ctx)?;
    if !policy.maker_posts() {
        return flow::create_pending_for(ctx, maker, kind).await;
    }
    let draft = mortsom_app_contract::OrderDraft {
        kind,
        ..flow::sell_draft(ctx)
    };
    let since = actions::relay_cursor(ctx);
    let order = actions::create_order(ctx, maker, &draft).await?;
    actions::wait_app_label(ctx, maker, &order, WAITING_BOND).await?;
    if prove_unpublished {
        actions::expect_relay_silent(ctx, &order, since, ctx.recovery.quiet_window).await?;
    }
    actions::pay_bond(ctx, maker, &order).await?;
    actions::wait_app_state(ctx, maker, &order, OrderState::Pending).await?;
    actions::wait_relay_state_since(
        ctx,
        &order,
        &[OrderState::Pending],
        since,
        ctx.state_timeout,
    )
    .await?;
    Ok(order)
}

/// `taker` takes `order` and is parked on the bond: their app shows the
/// bond window, while `maker`'s app and the book still show the order
/// pending for the whole quiet window. The daemon must not publish the
/// take before the bond locks, or a taker who never pays would block the
/// order for everyone. Returns the relay cursor from before the take.
async fn take_onto_bond(
    ctx: &Context,
    order: &OrderReference,
    taker: Actor,
    maker: Actor,
) -> Result<u64, ScenarioError> {
    let since = actions::relay_cursor(ctx);
    actions::take_order(ctx, taker, order).await?;
    actions::wait_app_label(ctx, taker, order, WAITING_BOND).await?;
    actions::wait_app_state(ctx, maker, order, OrderState::Pending).await?;
    actions::expect_relay_state_absent(
        ctx,
        order,
        OrderState::Taken,
        since,
        ctx.recovery.quiet_window,
    )
    .await?;
    Ok(since)
}

/// `taker` pays the bond from their node; the daemon publishes the take
/// once the bond locks, and not before.
async fn lock_bond(
    ctx: &Context,
    order: &OrderReference,
    taker: Actor,
    since: u64,
) -> Result<(), ScenarioError> {
    actions::pay_bond(ctx, taker, order).await?;
    actions::wait_relay_state_since(ctx, order, &[OrderState::Taken], since, ctx.state_timeout)
        .await?;
    Ok(())
}

/// Alice's sell order, taken by Bob whose bond locked: the buyer invoice
/// step is where the trade goes on from.
async fn sell_taken_with_bonds(ctx: &Context) -> Result<OrderReference, ScenarioError> {
    actions::require_room_for_bond_and_escrow(ctx)?;
    let order = create_bonded(ctx, Actor::Alice, OrderKind::Sell, false).await?;
    let since = take_onto_bond(ctx, &order, Actor::Bob, Actor::Alice).await?;
    lock_bond(ctx, &order, Actor::Bob, since).await?;
    Ok(order)
}

/// Bob's buy order, taken by Alice as seller whose bond locked: her app
/// then asks for the escrow.
async fn buy_taken_with_bonds(ctx: &Context) -> Result<OrderReference, ScenarioError> {
    actions::require_room_for_bond_and_escrow(ctx)?;
    let order = create_bonded(ctx, Actor::Bob, OrderKind::Buy, false).await?;
    let since = take_onto_bond(ctx, &order, Actor::Alice, Actor::Bob).await?;
    lock_bond(ctx, &order, Actor::Alice, since).await?;
    actions::wait_app_label(ctx, Actor::Alice, &order, WAITING_PAYMENT).await?;
    Ok(order)
}

/// Bob invoices and Alice funds; both apps show the trade active.
async fn invoice_and_fund(
    ctx: &Context,
    order: OrderReference,
) -> Result<flow::Taken, ScenarioError> {
    actions::wait_app_label(ctx, Actor::Bob, &order, WAITING_INVOICE).await?;
    let payout_hash = actions::provide_buyer_invoice(ctx, Actor::Bob, &order).await?;
    actions::wait_app_state(ctx, Actor::Alice, &order, OrderState::Taken).await?;
    let taken = flow::Taken { order, payout_hash };
    flow::fund(ctx, &taken.order).await?;
    Ok(taken)
}

/// `maker` cancels their pending order; their maker bond comes back.
async fn cancel_and_release_maker(
    ctx: &Context,
    maker: Actor,
    order: &OrderReference,
) -> Result<(), ScenarioError> {
    cancel_order(ctx, maker, order).await?;
    flow::expect_cancelled_before_active(ctx, maker, order).await?;
    actions::expect_bond_released(ctx, maker, order).await
}

/// Alice creates a sell order. Bob takes it and is asked for a bond; he
/// pays it from his node, the take is published, and the trade completes
/// as `happy_sell`. The hold settles once, the payout lands once, and both
/// bonds come back to their nodes.
pub async fn taker_bond_released_after_trade(ctx: &Context) -> Result<(), ScenarioError> {
    let order = sell_taken_with_bonds(ctx).await?;
    let taken = invoice_and_fund(ctx, order).await?;
    flow::fiat_sent(ctx, &taken.order).await?;
    flow::release_and_settle(ctx, &taken).await?;
    actions::expect_bond_released(ctx, Actor::Bob, &taken.order).await?;
    actions::expect_bond_released(ctx, Actor::Alice, &taken.order).await
}

/// Alice creates a sell order. Bob takes it, is asked for a bond, and
/// walks away instead of paying. His app no longer lists the order;
/// Alice's app and the book keep it pending for the whole quiet window,
/// so anyone else can take it. Alice then cancels it, and her bond comes
/// back.
pub async fn taker_bond_declined(ctx: &Context) -> Result<(), ScenarioError> {
    let order = create_bonded(ctx, Actor::Alice, OrderKind::Sell, false).await?;
    let since = take_onto_bond(ctx, &order, Actor::Bob, Actor::Alice).await?;
    actions::decline_bond(ctx, Actor::Bob, &order).await?;
    actions::expect_closed(ctx, Actor::Bob, &order, ctx.state_timeout).await?;
    actions::wait_app_state(ctx, Actor::Alice, &order, OrderState::Pending).await?;
    for state in [OrderState::Taken, OrderState::Cancelled] {
        actions::expect_relay_state_absent(ctx, &order, state, since, ctx.recovery.quiet_window)
            .await?;
    }
    cancel_and_release_maker(ctx, Actor::Alice, &order).await
}

/// Alice creates a sell order. Bob takes it, is asked for a bond, and
/// never pays it nor walks away; he is on the trade screen, not the pay
/// screen, when the bond invoice expires on the daemon's node. The order
/// never left `pending` on the book, the take was never published, and the
/// daemon now republishes it: a new pending revision, later than the take.
/// Alice's app shows it pending. The daemon tells Bob nothing, but the
/// revision comes with the invoice's expiry,
/// which his app knows: it no longer holds the order within a state wait of
/// it, wherever he is. Bob paid nothing. Alice then cancels the order, and
/// her bond comes back.
pub async fn taker_bond_expires_unpaid(ctx: &Context) -> Result<(), ScenarioError> {
    let order = create_bonded(ctx, Actor::Alice, OrderKind::Sell, false).await?;
    let since = take_onto_bond(ctx, &order, Actor::Bob, Actor::Alice).await?;
    actions::wait_relay_state_since(
        ctx,
        &order,
        &[OrderState::Pending],
        since,
        ctx.expiration_timeout,
    )
    .await?;
    actions::wait_app_state(ctx, Actor::Alice, &order, OrderState::Pending).await?;
    actions::expect_relay_state_absent(
        ctx,
        &order,
        OrderState::Taken,
        since,
        ctx.recovery.quiet_window,
    )
    .await?;
    actions::expect_closed(ctx, Actor::Bob, &order, ctx.state_timeout).await?;
    cancel_and_release_maker(ctx, Actor::Alice, &order).await
}

/// Bob creates a buy order. Alice takes it as seller and is asked for a
/// bond before the escrow: two hold invoices in a row, each paid from
/// her node. Once the bond locks the daemon asks for the escrow; once
/// the escrow is accepted it asks Bob for his invoice, and the trade
/// completes as `happy_buy`. Both bonds come back.
pub async fn seller_taker_bond_then_escrow(ctx: &Context) -> Result<(), ScenarioError> {
    let order = buy_taken_with_bonds(ctx).await?;
    actions::pay_hold_invoice(ctx, Actor::Alice, &order).await?;
    actions::wait_app_label(ctx, Actor::Bob, &order, WAITING_INVOICE).await?;
    let payout_hash = actions::provide_buyer_invoice(ctx, Actor::Bob, &order).await?;
    actions::wait_both(ctx, &order, OrderState::Active).await?;
    let taken = flow::Taken { order, payout_hash };
    flow::fiat_sent(ctx, &taken.order).await?;
    flow::release_and_settle(ctx, &taken).await?;
    actions::expect_bond_released(ctx, Actor::Alice, &taken.order).await?;
    actions::expect_bond_released(ctx, Actor::Bob, &taken.order).await
}

/// Alice creates a sell order and is asked for a bond before it is
/// published: her app parks the order on the bond, and the book carries
/// nothing for the whole quiet window. She pays from her node; the order
/// reaches the book pending. She cancels it, and the bond comes back.
pub async fn maker_bond_locked_before_publish(ctx: &Context) -> Result<(), ScenarioError> {
    require_maker_bond(ctx)?;
    let order = create_bonded(ctx, Actor::Alice, OrderKind::Sell, true).await?;
    cancel_and_release_maker(ctx, Actor::Alice, &order).await
}

/// Alice creates a sell order, is asked for a bond, and abandons the
/// order instead of paying. Her app drops it; the book never carries it.
/// Nothing was charged and nothing needs cleaning up.
pub async fn maker_bond_abandoned(ctx: &Context) -> Result<(), ScenarioError> {
    require_maker_bond(ctx)?;
    let since = actions::relay_cursor(ctx);
    let order = actions::create_order(ctx, Actor::Alice, &flow::sell_draft(ctx)).await?;
    actions::wait_app_label(ctx, Actor::Alice, &order, WAITING_BOND).await?;
    actions::decline_bond(ctx, Actor::Alice, &order).await?;
    actions::expect_closed(ctx, Actor::Alice, &order, ctx.state_timeout).await?;
    actions::expect_relay_silent(ctx, &order, since, ctx.recovery.quiet_window).await?;
    actions::mark_resolved(ctx, &order);
    Ok(())
}

/// Alice creates a sell order, is asked for a bond, and neither pays nor
/// walks away. Her app keeps the order parked on the bond for the whole
/// quiet window; once the maker's deadline passes the daemon drops the
/// order it never published, and her app no longer holds it. The book
/// never carries it, and nothing was charged.
pub async fn maker_bond_expires_unpaid(ctx: &Context) -> Result<(), ScenarioError> {
    require_maker_bond(ctx)?;
    let since = actions::relay_cursor(ctx);
    let order = actions::create_order(ctx, Actor::Alice, &flow::sell_draft(ctx)).await?;
    actions::wait_app_label(ctx, Actor::Alice, &order, WAITING_BOND).await?;
    actions::expect_state_holds(
        ctx,
        Actor::Alice,
        &order,
        OrderState::Taken,
        ctx.recovery.quiet_window,
    )
    .await?;
    // Still parked on the bond once the window is over: an app that let go
    // of the order early would pass the hold above unread.
    actions::wait_app_label(ctx, Actor::Alice, &order, WAITING_BOND).await?;
    actions::expect_closed(ctx, Actor::Alice, &order, ctx.expiration_timeout).await?;
    actions::expect_relay_silent(ctx, &order, since, ctx.recovery.quiet_window).await?;
    actions::mark_resolved(ctx, &order);
    Ok(())
}

/// Alice's sell order, taken and funded with both bonds locked, disputed
/// at fiat-sent by `opener` and taken by the run's solver.
async fn funded_trade_in_dispute(
    ctx: &Context,
    opener: Actor,
) -> Result<(flow::Taken, actions::Dispute), ScenarioError> {
    let order = sell_taken_with_bonds(ctx).await?;
    let taken = invoice_and_fund(ctx, order).await?;
    flow::fiat_sent(ctx, &taken.order).await?;
    let since = actions::relay_cursor(ctx);
    actions::open_dispute(ctx, opener, &taken.order).await?;
    let dispute = actions::wait_dispute_opened(ctx, since).await?;
    // Alice sells: which bond a ruling slashes follows from who is who.
    let expected = match opener {
        Actor::Alice => "seller",
        Actor::Bob => "buyer",
    };
    if dispute.initiator != expected {
        return Err(ScenarioError::product(format!(
            "the daemon records the dispute as opened by the {}, and the {expected} opened it",
            dispute.initiator
        )));
    }
    actions::wait_both(ctx, &taken.order, OrderState::Disputed).await?;
    let since = actions::relay_cursor(ctx);
    actions::solver_takes_dispute(ctx, &dispute, &taken.order).await?;
    actions::wait_dispute_status(
        ctx,
        &dispute.id,
        actions::dispute_status::IN_PROGRESS,
        since,
    )
    .await?;
    Ok((taken, dispute))
}

/// A funded, bonded trade in dispute: the solver settles it for Bob with
/// a resolution that slashes nobody. The escrow settles, the payout
/// lands, and both bonds come back.
pub async fn bonds_released_by_solver_ruling(ctx: &Context) -> Result<(), ScenarioError> {
    let (taken, dispute) = funded_trade_in_dispute(ctx, Actor::Alice).await?;
    let hash = taken
        .payout_hash
        .as_deref()
        .ok_or_else(|| ScenarioError::wallet("buyer payout hash is required before the ruling"))?;
    let since = actions::relay_cursor(ctx);
    actions::solver_rules_slashing(ctx, &taken.order, true, BondSlash::default()).await?;
    actions::wait_dispute_status(ctx, &dispute.id, actions::dispute_status::SETTLED, since).await?;
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
    actions::expect_bond_released(ctx, Actor::Bob, &taken.order).await?;
    actions::expect_bond_released(ctx, Actor::Alice, &taken.order).await?;
    actions::mark_resolved(ctx, &taken.order);
    Ok(())
}

crate::register_scenario! {
    name: "taker_bond_released_after_trade",
    body: taker_bond_released_after_trade,
    tags: &["bond"],
    requires: BOND_TRADE_REQUIRES,
    timeout_secs: TRADE_TIMEOUT_SECS,
    moves_value: true,
}
crate::register_scenario! {
    name: "taker_bond_declined",
    body: taker_bond_declined,
    tags: &["bond"],
    requires: BOND_TAKE_REQUIRES,
    timeout_secs: TRADE_TIMEOUT_SECS,
    moves_value: true,
}
crate::register_scenario! {
    name: "taker_bond_expires_unpaid",
    body: taker_bond_expires_unpaid,
    tags: &["bond"],
    requires: BOND_TAKE_REQUIRES,
    timeout_secs: SLASH_TIMEOUT_SECS,
    moves_value: true,
}
crate::register_scenario! {
    name: "seller_taker_bond_then_escrow",
    body: seller_taker_bond_then_escrow,
    tags: &["bond"],
    requires: BOND_TRADE_REQUIRES,
    timeout_secs: TRADE_TIMEOUT_SECS,
    moves_value: true,
}
crate::register_scenario! {
    name: "maker_bond_locked_before_publish",
    body: maker_bond_locked_before_publish,
    tags: &["bond"],
    requires: BOND_TAKE_REQUIRES,
    timeout_secs: TRADE_TIMEOUT_SECS,
    moves_value: true,
}
crate::register_scenario! {
    name: "maker_bond_abandoned",
    body: maker_bond_abandoned,
    tags: &["bond"],
    requires: BOND_TAKE_REQUIRES,
    timeout_secs: TRADE_TIMEOUT_SECS,
    moves_value: false,
}
crate::register_scenario! {
    name: "maker_bond_expires_unpaid",
    body: maker_bond_expires_unpaid,
    tags: &["bond"],
    requires: BOND_TAKE_REQUIRES,
    timeout_secs: SLASH_TIMEOUT_SECS,
    moves_value: false,
}
crate::register_scenario! {
    name: "bonds_released_by_solver_ruling",
    body: bonds_released_by_solver_ruling,
    tags: &["bond"],
    requires: BOND_DISPUTE_REQUIRES,
    timeout_secs: TRADE_TIMEOUT_SECS,
    moves_value: true,
    needs_app_wallet: false,
    needs_solver: true,
}

/// Keeps registrations linked.
pub fn link() {}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
