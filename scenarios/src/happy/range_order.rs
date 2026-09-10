//! An order with a fiat range, taken at an intermediate amount. The daemon
//! must price the take and, once the trade settles, publish the remainder
//! as a new order of the maker's, which the maker's app lists and can
//! cancel.

use mortsom_android_driver::avd::Actor;
use mortsom_app_contract::{CancelOutcome, Capability, OrderDraft, OrderKind, OrderReference};
use mortsom_scenario_kit::actions;
use mortsom_scenario_kit::cancel::cancel_order;
use mortsom_scenario_kit::context::{Context, ScenarioError};
use mortsom_scenario_kit::waits::wait_until;

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

/// The range spans one to three times the configured fiat amount and is
/// taken at two: the remainder equals the minimum, so the daemon spawns
/// it as an order for exactly that amount.
const RANGE_MAX_MULTIPLE: u64 = 3;
const TAKE_MULTIPLE: u64 = 2;

/// The scenario body.
pub async fn range_order(ctx: &Context) -> Result<(), ScenarioError> {
    let (min, max) = (ctx.fiat_amount, ctx.fiat_amount * RANGE_MAX_MULTIPLE);
    let taken_at = ctx.fiat_amount * TAKE_MULTIPLE;
    let draft = OrderDraft::range(
        OrderKind::Sell,
        ctx.fiat_code.clone(),
        min,
        max,
        ctx.payment_method.clone(),
    );
    let order = flow::create_pending_with(ctx, Actor::Alice, &draft).await?;
    expect_fiat_tag(ctx, &order, &[min, max]).await?;
    let listed_before = owned_orders(ctx, Actor::Alice).await?;

    let taken = flow::take_at(ctx, order, taken_at).await?;
    flow::fund(ctx, &taken.order).await?;
    flow::fiat_sent(ctx, &taken.order).await?;
    flow::release_and_settle(ctx, &taken).await?;

    // The remainder is a new pending order of the maker's: her app lists
    // it, the book carries it for what is left of the range, and she can
    // close it like any pending order of hers.
    let remainder = expect_remainder(ctx, &taken.order, &listed_before).await?;
    expect_fiat_tag(ctx, &remainder, &[max - taken_at]).await?;
    let outcome = cancel_order(ctx, Actor::Alice, &remainder).await?;
    if outcome != CancelOutcome::Cancelled {
        return Err(ScenarioError::product(format!(
            "remainder order {} could not be cancelled by its maker: {outcome:?}",
            remainder.id
        )));
    }
    actions::wait_app_state(
        ctx,
        Actor::Alice,
        &remainder,
        mortsom_app_contract::OrderState::Cancelled,
    )
    .await?;
    actions::wait_relay_state(ctx, &remainder, mortsom_app_contract::OrderState::Cancelled).await?;
    actions::mark_resolved(ctx, &remainder);
    Ok(())
}

/// The `fa` tag of the daemon's event for `order` must carry exactly
/// `expected`: both bounds while the range is open, one amount otherwise.
async fn expect_fiat_tag(
    ctx: &Context,
    order: &OrderReference,
    expected: &[u64],
) -> Result<(), ScenarioError> {
    let published = actions::relay_order_tag(ctx, order, "fa").await?;
    let expected: Vec<String> = expected.iter().map(u64::to_string).collect();
    if published != expected {
        return Err(ScenarioError::product(format!(
            "order {} was published for fiat [{}], expected [{}]",
            order.id,
            published.join(", "),
            expected.join(", ")
        )));
    }
    Ok(())
}

async fn owned_orders(ctx: &Context, actor: Actor) -> Result<Vec<String>, ScenarioError> {
    let session = ctx.actor(actor)?;
    Ok(ctx
        .adapter
        .owned_orders(&session.device(ctx.ui_timeout))
        .await?)
}

/// Waits for the maker's app to list one new order besides `parent` and
/// those in `known`, registers it for cleanup and scopes the observer to
/// it, then requires the relay to show it pending.
async fn expect_remainder(
    ctx: &Context,
    parent: &OrderReference,
    known: &[String],
) -> Result<OrderReference, ScenarioError> {
    let session = ctx.actor(Actor::Alice)?;
    let device = session.device(ctx.ui_timeout);
    let found = wait_until(ctx.state_timeout, actions::STATE_POLL, || async {
        match ctx.adapter.owned_orders(&device).await {
            Ok(ids) => ids
                .into_iter()
                .find(|id| id != &parent.id && !known.contains(id))
                .map(Ok),
            Err(e) => Some(Err(ScenarioError::from(e))),
        }
    })
    .await;
    let id = match found {
        Some(Ok(id)) => id,
        Some(Err(e)) => return Err(e),
        None => {
            return Err(ScenarioError::product(format!(
                "the maker's app lists no remainder of range order {} within {:?}",
                parent.id, ctx.state_timeout
            )));
        }
    };
    let remainder = OrderReference::with_kind(id.clone(), OrderKind::Sell);
    ctx.cleanup.register(Actor::Alice, &remainder);
    ctx.relay
        .add_order(&id)
        .await
        .map_err(|e| ScenarioError::relay(format!("cannot scope observer to order {id}: {e}")))?;
    actions::wait_app_state(
        ctx,
        Actor::Alice,
        &remainder,
        mortsom_app_contract::OrderState::Pending,
    )
    .await?;
    actions::wait_relay_state(ctx, &remainder, mortsom_app_contract::OrderState::Pending).await?;
    ctx.timeline.push(
        mortsom_scenario_kit::timeline::TimelineKind::Assertion,
        Some(Actor::Alice.as_str()),
        format!(
            "range order {} left remainder order {id}, pending for the maker",
            parent.id
        ),
        None,
    );
    Ok(remainder)
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
