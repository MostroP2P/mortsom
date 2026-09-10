//! The mirror of `range_order`: Bob publishes a **buy** order with a fiat
//! range and Alice takes it as seller at an intermediate amount. The buyer
//! names the remainder's trade key when reporting fiat sent, so after the
//! trade settles the remainder is a new pending order of Bob's.

use mortsom_android_driver::avd::Actor;
use mortsom_app_contract::{Capability, OrderDraft, OrderKind};
use mortsom_scenario_kit::context::{Context, ScenarioError};

use super::range;
use crate::flow;

/// Capabilities the scenario requires.
pub const REQUIRES: &[Capability] = super::range_order::REQUIRES;

/// The scenario body.
pub async fn range_buy_order(ctx: &Context) -> Result<(), ScenarioError> {
    let (min, max, taken_at) = range::bounds(ctx);
    let draft = OrderDraft::range(
        OrderKind::Buy,
        ctx.fiat_code.clone(),
        min,
        max,
        ctx.payment_method.clone(),
    );
    let order = flow::create_pending_with(ctx, Actor::Bob, &draft).await?;
    range::expect_fiat_tag(ctx, &order, &[min, max]).await?;
    let listed_before = range::owned_orders(ctx, Actor::Bob).await?;

    let taken = flow::take_buy_at_and_fund(ctx, order, taken_at).await?;
    flow::fiat_sent(ctx, &taken.order).await?;
    flow::release_and_settle(ctx, &taken).await?;

    range::settle_remainder(
        ctx,
        Actor::Bob,
        OrderKind::Buy,
        &taken.order,
        &listed_before,
        max - taken_at,
    )
    .await
}

crate::register_scenario! {
    name: "range_buy_order",
    body: range_buy_order,
    tags: &["happy-path"],
    requires: REQUIRES,
    timeout_secs: super::EXTENDED_TRADE_TIMEOUT_SECS,
    moves_value: true,
}

/// Keeps the registration linked.
pub fn link() {}
