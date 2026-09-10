//! A sell order with a fiat range, taken at an intermediate amount. The
//! daemon must price the take and, once the trade settles, publish the
//! remainder as a new order of the maker's, which the maker's app lists
//! and can cancel.

use mortsom_app_contract::{Capability, OrderDraft, OrderKind};
use mortsom_common::Actor;
use mortsom_scenario_kit::context::{Context, ScenarioError};

use super::range;
use crate::flow;

/// Capabilities the scenario requires.
pub const REQUIRES: &[Capability] = &[
    Capability::Onboarding,
    Capability::CustomNodeSelection,
    Capability::LocalRelayConfiguration,
    Capability::Nwc,
    Capability::CreateOrder,
    Capability::RangeOrder,
    Capability::TakeOrder,
    Capability::HoldInvoicePayment,
    Capability::BuyerInvoice,
    Capability::FiatSent,
    Capability::Release,
    Capability::CompletedObservation,
    Capability::Cancel,
];

/// The scenario body.
pub async fn range_order(ctx: &Context) -> Result<(), ScenarioError> {
    let (min, max, taken_at) = range::bounds(ctx);
    let draft = OrderDraft::range(
        OrderKind::Sell,
        ctx.fiat_code.clone(),
        min,
        max,
        ctx.payment_method.clone(),
    );
    let order = flow::create_pending_with(ctx, Actor::Alice, &draft).await?;
    range::expect_fiat_tag(ctx, &order, &[min, max]).await?;
    let listed_before = range::owned_orders(ctx, Actor::Alice).await?;

    let taken = flow::take_at(ctx, order, taken_at).await?;
    flow::fund(ctx, &taken.order).await?;
    flow::fiat_sent(ctx, &taken.order).await?;
    flow::release_and_settle(ctx, &taken).await?;

    range::settle_remainder(
        ctx,
        Actor::Alice,
        OrderKind::Sell,
        &taken.order,
        &listed_before,
        max - taken_at,
    )
    .await
}

crate::register_scenario! {
    name: "range_order",
    body: range_order,
    tags: &["happy-path"],
    requires: REQUIRES,
    timeout_secs: super::EXTENDED_TRADE_TIMEOUT_SECS,
    moves_value: true,
}

/// Keeps the registration linked.
pub fn link() {}
