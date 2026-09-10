//! An order priced at market rather than at a fixed sat amount: the daemon
//! resolves the rate when the order is taken, so the sats only exist once
//! the hold invoice does.

use mortsom_app_contract::{Capability, OrderDraft, OrderKind};
use mortsom_common::Actor;
use mortsom_scenario_kit::actions;
use mortsom_scenario_kit::context::{Context, ScenarioError};

use crate::flow;

/// Capabilities the scenario requires.
pub const REQUIRES: &[Capability] = &[
    Capability::Onboarding,
    Capability::CustomNodeSelection,
    Capability::LocalRelayConfiguration,
    Capability::Nwc,
    Capability::CreateOrder,
    Capability::MarketPriceOrder,
    Capability::TakeOrder,
    Capability::HoldInvoicePayment,
    Capability::BuyerInvoice,
    Capability::FiatSent,
    Capability::Release,
    Capability::CompletedObservation,
];

/// The book's sats amount of an order the daemon has not priced yet.
const UNPRICED: &str = "0";

/// The scenario body.
pub async fn market_price_order(ctx: &Context) -> Result<(), ScenarioError> {
    let draft = OrderDraft::market(
        OrderKind::Sell,
        ctx.fiat_code.clone(),
        ctx.fiat_amount,
        ctx.payment_method.clone(),
    );
    let order = flow::create_pending_with(ctx, Actor::Alice, &draft).await?;
    // A market order is published without sats: the daemon prices it at
    // the take. A book entry carrying an amount would be a fixed order the
    // app created out of a market draft.
    let published = actions::relay_order_tag(ctx, &order, "amt").await?;
    if published != [UNPRICED] {
        return Err(ScenarioError::product(format!(
            "market order {} was published with a sats amount of {}",
            order.id,
            published.join(",")
        )));
    }
    let taken = flow::take(ctx, order).await?;
    let hold_sats = flow::fund(ctx, &taken.order).await?;
    if hold_sats == 0 {
        return Err(ScenarioError::product(format!(
            "the daemon did not price market order {}: the hold invoice asks for nothing",
            taken.order.id
        )));
    }
    flow::fiat_sent(ctx, &taken.order).await?;
    flow::release_and_settle(ctx, &taken).await
}

crate::register_scenario! {
    name: "market_price_order",
    body: market_price_order,
    tags: &["happy-path"],
    requires: REQUIRES,
    timeout_secs: super::EXTENDED_TRADE_TIMEOUT_SECS,
    moves_value: true,
}

/// Keeps the registration linked.
pub fn link() {}
