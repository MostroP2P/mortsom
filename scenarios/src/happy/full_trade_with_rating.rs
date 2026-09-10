//! A happy trade followed by both parties rating each other, which is what
//! feeds the reputation system. The proof of each rating is the daemon's
//! own reputation event, not the star on the screen.

use mortsom_app_contract::{Capability, OrderReference};
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
    Capability::TakeOrder,
    Capability::HoldInvoicePayment,
    Capability::BuyerInvoice,
    Capability::FiatSent,
    Capability::Release,
    Capability::CompletedObservation,
    Capability::Rating,
];

/// The stars each party gives.
const SCORE: u8 = 5;
/// What the app shows for a completed trade that still awaits this user's
/// rating, and once it was given.
const AWAITING_RATING: &[&str] = &["pending-rating"];
const RATED: &[&str] = &["rated"];

/// The scenario body.
pub async fn full_trade_with_rating(ctx: &Context) -> Result<(), ScenarioError> {
    let taken = flow::create_and_take(ctx).await?;
    flow::fund(ctx, &taken.order).await?;
    flow::fiat_sent(ctx, &taken.order).await?;
    flow::release_and_settle(ctx, &taken).await?;
    for actor in Actor::ALL {
        rate(ctx, actor, &taken.order).await?;
    }
    Ok(())
}

/// `actor` is invited to rate once the payout is complete, rates, sees the
/// trade as rated, and the daemon publishes the reputation update.
async fn rate(ctx: &Context, actor: Actor, order: &OrderReference) -> Result<(), ScenarioError> {
    actions::wait_app_label(ctx, actor, order, AWAITING_RATING).await?;
    let since = actions::relay_cursor(ctx);
    actions::rate_counterpart(ctx, actor, order, SCORE).await?;
    actions::wait_app_label(ctx, actor, order, RATED).await?;
    actions::wait_rating_published(ctx, since).await?;
    Ok(())
}

crate::register_scenario! {
    name: "full_trade_with_rating",
    body: full_trade_with_rating,
    tags: &["happy-path"],
    requires: REQUIRES,
    timeout_secs: super::EXTENDED_TRADE_TIMEOUT_SECS,
    moves_value: true,
}

/// Keeps the registration linked.
pub fn link() {}
