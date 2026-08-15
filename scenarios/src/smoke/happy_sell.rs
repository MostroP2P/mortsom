//! `happy_sell` (spec §7, Phase 4): Alice sells, Bob buys, both apps show
//! every state, the relay agrees, and the payout settles.

use mortsom_android_driver::avd::Actor;
use mortsom_app_contract::{Capability, OrderDraft, OrderKind, OrderState};
use mortsom_scenario_kit::actions;
use mortsom_scenario_kit::context::{Context, ScenarioError};
use mortsom_scenario_kit::registry::{ScenarioDef, ScenarioFuture};

/// Capabilities the scenario requires (spec §12.2).
pub const REQUIRES: &[Capability] = &[
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

/// The scenario body.
pub async fn happy_sell(ctx: &Context) -> Result<(), ScenarioError> {
    let draft = OrderDraft {
        kind: OrderKind::Sell,
        amount_sats: ctx.amount_sats,
        fiat_code: ctx.fiat_code.clone(),
        fiat_amount: ctx.fiat_amount,
        payment_method: ctx.payment_method.clone(),
    };
    let order = actions::create_order(ctx, Actor::Alice, &draft).await?;
    actions::wait_app_state(ctx, Actor::Alice, &order, OrderState::Pending).await?;
    actions::wait_relay_state(ctx, &order, OrderState::Pending).await?;

    actions::take_order(ctx, Actor::Bob, &order).await?;
    actions::wait_both(ctx, &order, OrderState::Taken).await?;

    let payout_hash = actions::provide_buyer_invoice(ctx, Actor::Bob, &order).await?;
    actions::wait_app_state(ctx, Actor::Alice, &order, OrderState::Taken).await?;

    actions::pay_hold_invoice(ctx, Actor::Alice, &order).await?;
    actions::wait_both(ctx, &order, OrderState::Active).await?;
    actions::wait_relay_state(ctx, &order, OrderState::Active).await?;

    actions::confirm_fiat_sent(ctx, Actor::Bob, &order).await?;
    actions::wait_app_state(ctx, Actor::Alice, &order, OrderState::FiatSent).await?;

    actions::release(ctx, Actor::Alice, &order).await?;
    actions::wait_both(ctx, &order, OrderState::Completed).await?;
    actions::wait_relay_state(ctx, &order, OrderState::Completed).await?;
    if let Some(hash) = payout_hash {
        actions::expect_payout_settled(ctx, Actor::Bob, &order, &hash).await?;
    }
    actions::mark_resolved(ctx, &order);
    Ok(())
}

fn run(ctx: &Context) -> ScenarioFuture<'_> {
    Box::pin(happy_sell(ctx))
}

inventory::submit! {
    ScenarioDef {
        name: "happy_sell",
        tags: &["smoke", "happy-path"],
        requires: REQUIRES,
        timeout_secs: 30 * 60,
        moves_value: true,
        run,
    }
}

/// Keeps the registration linked.
pub fn link() {}
