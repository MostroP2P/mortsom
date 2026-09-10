//! What every range order proves: the book advertises both bounds, the
//! daemon prices the take, and once the trade settles the remainder is a
//! new pending order of the maker's, listed by the maker's app, published
//! for the fiat left of the range, and closed by the maker like any other
//! pending order.

use mortsom_app_contract::{CancelOutcome, OrderKind, OrderReference, OrderState};
use mortsom_common::Actor;
use mortsom_scenario_kit::actions;
use mortsom_scenario_kit::cancel::cancel_order;
use mortsom_scenario_kit::context::{Context, ScenarioError};
use mortsom_scenario_kit::timeline::TimelineKind;
use mortsom_scenario_kit::waits::wait_until;

/// The range spans one to three times the configured fiat amount and is
/// taken at two: the remainder equals the minimum, so the daemon spawns
/// it as an order for exactly that amount.
pub const RANGE_MAX_MULTIPLE: u64 = 3;
/// Multiple of the configured fiat amount the taker picks.
pub const TAKE_MULTIPLE: u64 = 2;

/// The fiat bounds and the take amount of every range scenario.
pub fn bounds(ctx: &Context) -> (u64, u64, u64) {
    (
        ctx.fiat_amount,
        ctx.fiat_amount * RANGE_MAX_MULTIPLE,
        ctx.fiat_amount * TAKE_MULTIPLE,
    )
}

/// The `fa` tag of the daemon's event for `order` must carry exactly
/// `expected`: both bounds while the range is open, one amount otherwise.
pub async fn expect_fiat_tag(
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

/// The order ids `actor`'s app lists as its own.
pub async fn owned_orders(ctx: &Context, actor: Actor) -> Result<Vec<String>, ScenarioError> {
    let session = ctx.actor(actor)?;
    Ok(ctx
        .adapter
        .owned_orders(&session.device(ctx.ui_timeout))
        .await?)
}

/// After the trade settled: the remainder is a new pending order of
/// `maker`'s, listed by their app, on the book for `fiat_left`, and closed
/// by the maker with relay confirmation. Marks it resolved.
pub async fn settle_remainder(
    ctx: &Context,
    maker: Actor,
    kind: OrderKind,
    parent: &OrderReference,
    known: &[String],
    fiat_left: u64,
) -> Result<(), ScenarioError> {
    let remainder = expect_remainder(ctx, maker, kind, parent, known).await?;
    expect_fiat_tag(ctx, &remainder, &[fiat_left]).await?;
    let outcome = cancel_order(ctx, maker, &remainder).await?;
    if outcome != CancelOutcome::Cancelled {
        return Err(ScenarioError::product(format!(
            "remainder order {} could not be cancelled by its maker: {outcome:?}",
            remainder.id
        )));
    }
    actions::wait_app_state(ctx, maker, &remainder, OrderState::Cancelled).await?;
    actions::wait_relay_state(ctx, &remainder, OrderState::Cancelled).await?;
    actions::mark_resolved(ctx, &remainder);
    Ok(())
}

/// Waits for `maker`'s app to list one new order besides `parent` and
/// those in `known`, registers it for cleanup and scopes the observer to
/// it, then requires both the app and the relay to show it pending.
async fn expect_remainder(
    ctx: &Context,
    maker: Actor,
    kind: OrderKind,
    parent: &OrderReference,
    known: &[String],
) -> Result<OrderReference, ScenarioError> {
    let session = ctx.actor(maker)?;
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
    let remainder = OrderReference::with_kind(id.clone(), kind);
    ctx.cleanup.register(maker, &remainder);
    ctx.relay
        .add_order(&id)
        .await
        .map_err(|e| ScenarioError::relay(format!("cannot scope observer to order {id}: {e}")))?;
    actions::wait_app_state(ctx, maker, &remainder, OrderState::Pending).await?;
    actions::wait_relay_state(ctx, &remainder, OrderState::Pending).await?;
    ctx.timeline.push(
        TimelineKind::Assertion,
        Some(maker.as_str()),
        format!(
            "range order {} left remainder order {id}, pending for the maker",
            parent.id
        ),
        None,
    );
    Ok(remainder)
}
