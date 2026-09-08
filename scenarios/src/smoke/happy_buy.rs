//! Bob creates a buy order and Alice takes as seller. Escrow acceptance
//! precedes the buyer invoice; final settlement shares the sell assertions.

use mortsom_app_contract::Capability;
use mortsom_scenario_kit::context::{Context, ScenarioError};

use crate::flow;

/// Capabilities the scenario requires.
pub const REQUIRES: &[Capability] = flow::TRADE_REQUIRES;

/// The scenario body.
pub async fn happy_buy(ctx: &Context) -> Result<(), ScenarioError> {
    let taken = flow::create_buy_and_fund(ctx).await?;
    flow::fiat_sent(ctx, &taken.order).await?;
    flow::release_and_settle(ctx, &taken).await
}

crate::register_scenario! {
    name: "happy_buy",
    body: happy_buy,
    tags: &["smoke", "happy-path", "parity"],
    requires: REQUIRES,
    timeout_secs: 30 * 60,
    moves_value: true,
}

/// Keeps the registration linked.
pub fn link() {}
