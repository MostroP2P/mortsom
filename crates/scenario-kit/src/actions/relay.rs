//! Relay observations beyond "the order reached this state": what the
//! daemon publishes *after* a point in the run, and what it must not.
use std::time::Duration;

use mortsom_app_contract::{OrderReference, OrderState};

use crate::context::{Context, ScenarioError};
use crate::timeline::TimelineKind;

use super::{ORDER_EVENT_KIND, short};

/// The relay sequence now: a baseline for observations that must not be
/// satisfied by an event the daemon published earlier.
pub fn relay_cursor(ctx: &Context) -> u64 {
    ctx.relay.last_seq()
}

/// Waits until the daemon publishes `order` in one of `accepted` after
/// `since`, bounded by `timeout`, and returns which. The order's earlier
/// revisions do not count: an order that was pending, was taken and is
/// pending again is proved by the second `pending`, never the first.
pub async fn wait_relay_state_since(
    ctx: &Context,
    order: &OrderReference,
    accepted: &[OrderState],
    since: u64,
    timeout: Duration,
) -> Result<OrderState, ScenarioError> {
    let id = order.id.clone();
    let event = ctx
        .relay
        .wait_for(timeout, |e| {
            e.seq > since
                && e.kind == ORDER_EVENT_KIND
                && e.tag("d") == Some(id.as_str())
                && e.tag("s")
                    .and_then(OrderState::from_protocol_status)
                    .is_some_and(|state| accepted.contains(&state))
        })
        .await
        .ok_or_else(|| {
            ScenarioError::relay(format!(
                "no relay event after the baseline shows order {} as {} within {timeout:?}",
                order.id,
                super::names(accepted)
            ))
        })?;
    let state = event
        .tag("s")
        .and_then(OrderState::from_protocol_status)
        .ok_or_else(|| ScenarioError::relay("matched relay event lost its status"))?;
    // The parity report reads the relay path in this grammar
    // (`coordinator::parity::relay_path`).
    ctx.timeline.push_at(
        event.received_at_ms,
        TimelineKind::RelayEvent,
        None,
        format!(
            "relay: order {} is {state} (event {})",
            order.id,
            short(&event.id)
        ),
        Some(event.id.clone()),
    );
    Ok(state)
}

/// Negative correlation: the daemon must not publish `order` as `state`
/// after `since` for the whole `window`. The proof of "nothing happened"
/// is the window spent watching, so this always takes `window`.
pub async fn expect_relay_state_absent(
    ctx: &Context,
    order: &OrderReference,
    state: OrderState,
    since: u64,
    window: Duration,
) -> Result<(), ScenarioError> {
    let id = order.id.clone();
    let found = ctx
        .relay
        .wait_for(window, |e| {
            e.seq > since
                && e.kind == ORDER_EVENT_KIND
                && e.tag("d") == Some(id.as_str())
                && e.tag("s").and_then(OrderState::from_protocol_status) == Some(state)
        })
        .await;
    match found {
        None => {
            ctx.timeline.push(
                TimelineKind::Assertion,
                None,
                format!(
                    "relay: the daemon did not publish order {} as {state} for {window:?}",
                    order.id
                ),
                None,
            );
            Ok(())
        }
        Some(event) => Err(ScenarioError::product(format!(
            "the daemon published order {} as {state} within {window:?} (event {})",
            order.id,
            short(&event.id)
        ))),
    }
}

/// Negative correlation: the daemon must publish nothing about `order`
/// after `since`, in any status, for the whole `window`. For an order the
/// daemon never puts on the book: a maker's, parked on an unpaid bond.
pub async fn expect_relay_silent(
    ctx: &Context,
    order: &OrderReference,
    since: u64,
    window: Duration,
) -> Result<(), ScenarioError> {
    let id = order.id.clone();
    let found = ctx
        .relay
        .wait_for(window, |e| {
            e.seq > since && e.kind == ORDER_EVENT_KIND && e.tag("d") == Some(id.as_str())
        })
        .await;
    match found {
        None => {
            ctx.timeline.push(
                TimelineKind::Assertion,
                None,
                format!(
                    "relay: the daemon published nothing about order {} for {window:?}",
                    order.id
                ),
                None,
            );
            Ok(())
        }
        Some(event) => Err(ScenarioError::product(format!(
            "the daemon published order {} as `{}` (event {}); it never reached the book",
            order.id,
            event.tag("s").unwrap_or_default(),
            short(&event.id)
        ))),
    }
}

/// The values of a tag on the daemon's newest event for `order`, waiting
/// for a first one when none exists yet: the `fa` tag carries the fiat
/// amount, or a range's two bounds.
pub async fn relay_order_tag(
    ctx: &Context,
    order: &OrderReference,
    tag: &str,
) -> Result<Vec<String>, ScenarioError> {
    let id = order.id.clone();
    let is_order = |e: &mortsom_observers::relay::ObservedEvent| {
        e.kind == ORDER_EVENT_KIND && e.tag("d") == Some(id.as_str())
    };
    let newest = ctx.relay.snapshot().into_iter().rev().find(is_order);
    let event = match newest {
        Some(event) => Some(event),
        None => ctx.relay.wait_for(ctx.state_timeout, is_order).await,
    }
    .ok_or_else(|| {
        ScenarioError::relay(format!(
            "no relay event for order {} within {:?}",
            order.id, ctx.state_timeout
        ))
    })?;
    let values = event
        .tags
        .iter()
        .find(|t| t.first().map(String::as_str) == Some(tag))
        .map(|t| t[1..].to_vec())
        .ok_or_else(|| {
            ScenarioError::relay(format!(
                "the daemon's event for order {} carries no `{tag}` tag",
                order.id
            ))
        })?;
    Ok(values)
}
