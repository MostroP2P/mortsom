//! `happy_sell` (spec §7, Phase 4): Alice sells, Bob buys, both apps show
//! every state, the relay agrees, and the payout settles.

use mortsom_app_contract::Capability;
use mortsom_scenario_kit::context::{Context, ScenarioError};

use crate::flow;

/// Capabilities the scenario requires (spec §12.2).
pub const REQUIRES: &[Capability] = flow::TRADE_REQUIRES;

/// The scenario body.
pub async fn happy_sell(ctx: &Context) -> Result<(), ScenarioError> {
    let taken = flow::create_and_take(ctx).await?;
    flow::fund(ctx, &taken.order).await?;
    flow::fiat_sent(ctx, &taken.order).await?;
    flow::release_and_settle(ctx, &taken).await
}

crate::register_scenario! {
    name: "happy_sell",
    body: happy_sell,
    tags: &["smoke", "happy-path", "parity"],
    requires: REQUIRES,
    timeout_secs: 30 * 60,
    moves_value: true,
}

/// Keeps the registration linked.
pub fn link() {}
