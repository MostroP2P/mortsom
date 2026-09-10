//! Shared trade journeys (spec §7): the steps every scenario composes, each
//! finishing only when the expected visible state and, where applicable,
//! the relay agree. Scenarios differ in what they interrupt in between.

use mortsom_app_contract::{Capability, OrderDraft, OrderKind, OrderReference, OrderState};
use mortsom_common::Actor;
use mortsom_scenario_kit::actions;
use mortsom_scenario_kit::context::{Context, ScenarioError};

/// Capabilities needed to reach a taken order without moving value.
pub const TAKE_REQUIRES: &[Capability] = &[
    Capability::Onboarding,
    Capability::CustomNodeSelection,
    Capability::LocalRelayConfiguration,
    Capability::Nwc,
    Capability::CreateOrder,
    Capability::TakeOrder,
    Capability::BuyerInvoice,
];

/// Capabilities needed to complete a trade end to end.
pub const TRADE_REQUIRES: &[Capability] = &[
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
];

/// Capabilities needed to cancel.
pub const CANCEL_REQUIRES: &[Capability] = &[
    Capability::Onboarding,
    Capability::CustomNodeSelection,
    Capability::LocalRelayConfiguration,
    Capability::Nwc,
    Capability::CreateOrder,
    Capability::TakeOrder,
    Capability::BuyerInvoice,
    Capability::Cancel,
];

/// A taken order with the buyer's payout hash, when observable.
pub struct Taken {
    /// The order.
    pub order: OrderReference,
    /// Payment hash of the buyer invoice, when the app exposed it.
    pub payout_hash: Option<String>,
}

/// The fixed-price sell draft every ordinary scenario uses.
pub fn sell_draft(ctx: &Context) -> OrderDraft {
    OrderDraft::fixed(
        OrderKind::Sell,
        ctx.amount_sats,
        ctx.fiat_code.clone(),
        ctx.fiat_amount,
        ctx.payment_method.clone(),
    )
}

/// Alice publishes a sell order; her app and the relay show it pending.
pub async fn create_pending(ctx: &Context) -> Result<OrderReference, ScenarioError> {
    create_pending_for(ctx, Actor::Alice, OrderKind::Sell).await
}

async fn create_pending_for(
    ctx: &Context,
    maker: Actor,
    kind: OrderKind,
) -> Result<OrderReference, ScenarioError> {
    let draft = OrderDraft {
        kind,
        ..sell_draft(ctx)
    };
    create_pending_with(ctx, maker, &draft).await
}

/// `maker` publishes `draft`; their app and the relay show it pending.
pub async fn create_pending_with(
    ctx: &Context,
    maker: Actor,
    draft: &OrderDraft,
) -> Result<OrderReference, ScenarioError> {
    let order = actions::create_order(ctx, maker, draft).await?;
    actions::wait_app_state(ctx, maker, &order, OrderState::Pending).await?;
    actions::wait_relay_state(ctx, &order, OrderState::Pending).await?;
    Ok(order)
}

/// Bob takes `order` and hands in his invoice; both apps show it taken
/// (waiting for the seller's payment). No value moves.
pub async fn take(ctx: &Context, order: OrderReference) -> Result<Taken, ScenarioError> {
    actions::take_order(ctx, Actor::Bob, &order).await?;
    actions::wait_both(ctx, &order, OrderState::Taken).await?;
    let payout_hash = actions::provide_buyer_invoice(ctx, Actor::Bob, &order).await?;
    actions::wait_app_state(ctx, Actor::Alice, &order, OrderState::Taken).await?;
    Ok(Taken { order, payout_hash })
}

/// Create and take in one go.
pub async fn create_and_take(ctx: &Context) -> Result<Taken, ScenarioError> {
    let order = create_pending(ctx).await?;
    take(ctx, order).await
}

/// Bob takes the range `order` at `fiat_amount` and hands in his invoice
/// for the sats the daemon priced; both apps show it taken. No value moves.
pub async fn take_at(
    ctx: &Context,
    order: OrderReference,
    fiat_amount: u64,
) -> Result<Taken, ScenarioError> {
    actions::take_order_at(ctx, Actor::Bob, &order, fiat_amount).await?;
    actions::wait_both(ctx, &order, OrderState::Taken).await?;
    let payout_hash = actions::provide_buyer_invoice(ctx, Actor::Bob, &order).await?;
    actions::wait_app_state(ctx, Actor::Alice, &order, OrderState::Taken).await?;
    Ok(Taken { order, payout_hash })
}

/// Alice creates a sell order, Bob takes it, Alice funds it: the order both
/// apps show active, with the escrow held. Where every deviation from the
/// happy path that needs a funded order starts.
pub async fn create_take_and_fund(ctx: &Context) -> Result<Taken, ScenarioError> {
    let taken = create_and_take(ctx).await?;
    fund(ctx, &taken.order).await?;
    Ok(taken)
}

/// Bob publishes a buy and Alice takes it as seller. Her escrow must be
/// accepted before Bob submits his payout invoice and both apps become active.
pub async fn create_buy_and_fund(ctx: &Context) -> Result<Taken, ScenarioError> {
    let order = create_pending_for(ctx, Actor::Bob, OrderKind::Buy).await?;
    actions::take_order(ctx, Actor::Alice, &order).await?;
    fund_taken_buy(ctx, order).await
}

/// Alice takes Bob's range buy `order` at `fiat_amount` as seller and funds
/// it; Bob then submits his payout invoice and both apps become active.
pub async fn take_buy_at_and_fund(
    ctx: &Context,
    order: OrderReference,
    fiat_amount: u64,
) -> Result<Taken, ScenarioError> {
    actions::take_order_at(ctx, Actor::Alice, &order, fiat_amount).await?;
    fund_taken_buy(ctx, order).await
}

/// The seller side of a taken buy: Alice's escrow, then Bob's invoice.
async fn fund_taken_buy(ctx: &Context, order: OrderReference) -> Result<Taken, ScenarioError> {
    actions::wait_both(ctx, &order, OrderState::Taken).await?;
    actions::pay_hold_invoice(ctx, Actor::Alice, &order).await?;
    // The daemon asks the buyer for the payout invoice only once the hold is
    // accepted, and the app rebuilds the invoice form when that request
    // lands. An invoice typed before it is dropped, so wait for the ask.
    actions::wait_app_label(
        ctx,
        Actor::Bob,
        &order,
        &["waiting-invoice", "waiting-buyer-invoice"],
    )
    .await?;
    let payout_hash = actions::provide_buyer_invoice(ctx, Actor::Bob, &order).await?;
    actions::wait_both(ctx, &order, OrderState::Active).await?;
    actions::wait_relay_state(ctx, &order, OrderState::Taken).await?;
    Ok(Taken { order, payout_hash })
}

/// Alice pays the hold invoice; both apps show the order active. The daemon
/// tells only the two parties about `active`: the public book keeps the
/// order `in-progress` from the take until it closes, so that is the relay
/// state to correlate with. Moves value: requires mainnet clearance.
/// Returns the sats the hold invoice asked for.
pub async fn fund(ctx: &Context, order: &OrderReference) -> Result<u64, ScenarioError> {
    let hold = actions::pay_hold_invoice(ctx, Actor::Alice, order).await?;
    actions::wait_both(ctx, order, OrderState::Active).await?;
    actions::wait_relay_state(ctx, order, OrderState::Taken).await?;
    Ok(hold.amount_sats)
}

/// Bob reports fiat sent; Alice sees it.
pub async fn fiat_sent(ctx: &Context, order: &OrderReference) -> Result<(), ScenarioError> {
    actions::confirm_fiat_sent(ctx, Actor::Bob, order).await?;
    actions::wait_app_state(ctx, Actor::Alice, order, OrderState::FiatSent)
        .await
        .map(|_| ())
}

/// Alice releases; both apps and the relay show completion, the held HTLC
/// settles (on the node path, this is where funding is finally committed
/// against the budget) and Bob's wallet
/// shows it settled. Marks the order resolved for cleanup.
pub async fn release_and_settle(ctx: &Context, taken: &Taken) -> Result<(), ScenarioError> {
    let hash = taken.payout_hash.as_deref().ok_or_else(|| {
        ScenarioError::wallet("buyer payout hash is required before releasing escrow")
    })?;
    actions::release(ctx, Actor::Alice, &taken.order).await?;
    actions::wait_both(ctx, &taken.order, OrderState::Completed).await?;
    actions::wait_relay_state(ctx, &taken.order, OrderState::Completed).await?;
    actions::settle_hold_payment(ctx, &taken.order).await?;
    actions::expect_payout_settled(ctx, Actor::Bob, &taken.order, hash).await?;
    actions::mark_resolved(ctx, &taken.order);
    Ok(())
}

/// Every app in `participants` and the relay agree the order is cancelled
/// and, when a hold payment was in flight, the escrow came back; marks it
/// resolved. An actor who never took the order does not list it, so only
/// the parties to it are asked: the relay is the witness for everyone
/// else.
pub async fn expect_cancelled(
    ctx: &Context,
    order: &OrderReference,
    participants: &[Actor],
) -> Result<(), ScenarioError> {
    for actor in participants {
        actions::wait_app_state(ctx, *actor, order, OrderState::Cancelled).await?;
    }
    actions::wait_relay_state(ctx, order, OrderState::Cancelled).await?;
    actions::expect_hold_refunded(ctx, order).await?;
    actions::mark_resolved(ctx, order);
    Ok(())
}

#[cfg(test)]
#[path = "flow_tests.rs"]
mod tests;
