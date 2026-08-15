//! Shared trade journeys (spec §7): the steps every scenario composes, each
//! finishing only when the expected visible state and, where applicable,
//! the relay agree. Scenarios differ in what they interrupt in between.

use mortsom_android_driver::avd::Actor;
use mortsom_app_contract::{Capability, OrderDraft, OrderKind, OrderReference, OrderState};
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

/// The sell draft every scenario uses.
pub fn sell_draft(ctx: &Context) -> OrderDraft {
    OrderDraft {
        kind: OrderKind::Sell,
        amount_sats: ctx.amount_sats,
        fiat_code: ctx.fiat_code.clone(),
        fiat_amount: ctx.fiat_amount,
        payment_method: ctx.payment_method.clone(),
    }
}

/// Alice publishes a sell order; her app and the relay show it pending.
pub async fn create_pending(ctx: &Context) -> Result<OrderReference, ScenarioError> {
    let order = actions::create_order(ctx, Actor::Alice, &sell_draft(ctx)).await?;
    actions::wait_app_state(ctx, Actor::Alice, &order, OrderState::Pending).await?;
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

/// Alice pays the hold invoice; both apps and the relay show the order
/// active. Moves value: requires mainnet clearance.
pub async fn fund(ctx: &Context, order: &OrderReference) -> Result<(), ScenarioError> {
    actions::pay_hold_invoice(ctx, Actor::Alice, order).await?;
    actions::wait_both(ctx, order, OrderState::Active).await?;
    actions::wait_relay_state(ctx, order, OrderState::Active).await
}

/// Bob reports fiat sent; Alice sees it.
pub async fn fiat_sent(ctx: &Context, order: &OrderReference) -> Result<(), ScenarioError> {
    actions::confirm_fiat_sent(ctx, Actor::Bob, order).await?;
    actions::wait_app_state(ctx, Actor::Alice, order, OrderState::FiatSent)
        .await
        .map(|_| ())
}

/// Alice releases; both apps and the relay show completion and, when the
/// payout hash is known, Bob's wallet shows it settled. Marks the order
/// resolved for cleanup.
pub async fn release_and_settle(ctx: &Context, taken: &Taken) -> Result<(), ScenarioError> {
    actions::release(ctx, Actor::Alice, &taken.order).await?;
    actions::wait_both(ctx, &taken.order, OrderState::Completed).await?;
    actions::wait_relay_state(ctx, &taken.order, OrderState::Completed).await?;
    if let Some(hash) = &taken.payout_hash {
        actions::expect_payout_settled(ctx, Actor::Bob, &taken.order, hash).await?;
    }
    actions::mark_resolved(ctx, &taken.order);
    Ok(())
}

/// Both apps and the relay agree the order is cancelled; marks it resolved.
pub async fn expect_cancelled(ctx: &Context, order: &OrderReference) -> Result<(), ScenarioError> {
    actions::wait_both(ctx, order, OrderState::Cancelled).await?;
    actions::wait_relay_state(ctx, order, OrderState::Cancelled).await?;
    actions::mark_resolved(ctx, order);
    Ok(())
}
