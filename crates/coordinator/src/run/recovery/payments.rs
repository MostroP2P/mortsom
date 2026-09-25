//! Correlates terminal public UI states with exact run-owned Lightning records.
use super::*;
use mortsom_app_contract::{OrderReference, OrderState};
use mortsom_observers::wallet::PaymentState;

pub(in crate::run) async fn verify_payments(
    ctx: &Context,
    journal: &FileJournal,
) -> Result<(), String> {
    let events = journal.snapshot().events;
    for event in &events {
        let RecoveryEvent::PaymentExpected {
            order_id,
            wallet,
            hash,
            incoming,
            amount_sats,
        } = event
        else {
            continue;
        };
        let state = terminal_order_state(ctx, &events, order_id).await?;
        let node = ctx
            .node_wallets
            .get(wallet)
            .ok_or("payment recovery requires the original node wallet")?;
        // A payment the scenario withdrew on purpose (a refused or replaced
        // payout invoice) must never have settled, whatever the order came
        // to; the node is asked to cancel it again, which is idempotent.
        let withdrawn = *incoming && crate::recovery_store::superseded(&events, wallet, hash);
        if withdrawn || (*incoming && matches!(state, OrderState::Cancelled | OrderState::Expired))
        {
            node.cancel_invoice(hash).await?;
        }
        let payment = ctx
            .wallets
            .wait_terminal(wallet, order_id, hash, ctx.settle_timeout)
            .await
            .map_err(|e| e.to_string())?;
        let count = node.settled_record_count(hash, *incoming).await?;
        let safe = if withdrawn {
            payment.state == PaymentState::Failed && count == 0
        } else if state == OrderState::Completed {
            payment.state == PaymentState::Settled
                && payment.amount_sats == *amount_sats
                && if *incoming {
                    payment.fee_sats == 0
                } else {
                    payment.fee_sats <= node.routing_fee_cap_sats()
                }
                && count == 1
        } else {
            payment.state == PaymentState::Failed && count == 0
        };
        if !safe {
            return Err(format!(
                "order {order_id} retains an unresolved or mismatched wallet outcome for hash {hash}"
            ));
        }
        ctx.timeline.push(
            mortsom_scenario_kit::timeline::TimelineKind::Assertion,
            None,
            format!("wallet {wallet} order {order_id} hash {hash}: incoming={incoming}, withdrawn={withdrawn}, principal={} sats, fee={} sats, settled ledger records={count}", payment.amount_sats, payment.fee_sats),
            Some(hash.clone()),
        );
    }
    Ok(())
}

async fn terminal_order_state(
    ctx: &Context,
    events: &[RecoveryEvent],
    order_id: &str,
) -> Result<OrderState, String> {
    let order = OrderReference::new(order_id);
    let maker = events
        .iter()
        .find_map(|event| match event {
            RecoveryEvent::OrderFound { actor, order } if order.id == order_id => Some(*actor),
            _ => None,
        })
        .ok_or("payment recovery has no known order owner")?;
    let session = ctx.actor(maker).map_err(|e| e.to_string())?;
    let device = session.device(ctx.ui_timeout);
    let state = match ctx.adapter.observe_order(&device, &order).await {
        Ok(shown) => shown.state,
        Err(unread) => return closed_off_the_list(ctx, &device, &order, unread.to_string()).await,
    };
    if matches!(state, OrderState::Completed | OrderState::PayoutPending) {
        for actor in Actor::ALL {
            let shown = mortsom_scenario_kit::actions::wait_app_state(
                ctx,
                actor,
                &order,
                OrderState::Completed,
            )
            .await;
            let Err(lagging) = shown else { continue };
            // The daemon tells only the buyer about the completed payout;
            // the seller's client learns it from the public book, and one
            // that missed that event keeps showing payout pending. The
            // daemon's own `success`, plus the exact settlement records
            // checked next, decide whether funds are still open; a lagging
            // screen is recorded, not obeyed.
            let _ = ctx.relay.add_order(order_id).await;
            mortsom_scenario_kit::actions::wait_relay_state(ctx, &order, OrderState::Completed)
                .await
                .map_err(|e| format!("{lagging}; and {e}"))?;
            ctx.timeline.push(
                mortsom_scenario_kit::timeline::TimelineKind::Assertion,
                Some(actor.as_str()),
                format!(
                    "order {order_id}: {} never showed completed ({lagging}); the daemon published success, \
                     settlement records decide",
                    actor.as_str()
                ),
                None,
            );
        }
        return Ok(OrderState::Completed);
    }
    if matches!(state, OrderState::Cancelled | OrderState::Expired) {
        return Ok(state);
    }
    Err(format!(
        "order {order_id} has no verified terminal UI state"
    ))
}

/// The terminal state of an order the maker's app could not show. App v2
/// drops a trade that never went active from My Trades once it is
/// cancelled, so its state goes with the row: only when the loaded list no
/// longer shows it does the daemon's own status on the relay decide, and
/// only a cancel or an expiry can, since a completed trade stays listed.
async fn closed_off_the_list(
    ctx: &Context,
    device: &mortsom_app_contract::DeviceContext<'_>,
    order: &OrderReference,
    unread: String,
) -> Result<OrderState, String> {
    let listed = ctx
        .adapter
        .owned_orders(device)
        .await
        .map_err(|e| e.to_string())?;
    if listed.contains(&order.id) {
        return Err(unread);
    }
    let _ = ctx.relay.add_order(&order.id).await;
    let id = order.id.clone();
    let closed = ctx
        .relay
        .wait_for(ctx.state_timeout, |e| {
            e.kind == mortsom_scenario_kit::actions::ORDER_EVENT_KIND
                && e.tag("d") == Some(id.as_str())
                && e.tag("s")
                    .and_then(OrderState::from_protocol_status)
                    .is_some_and(|state| {
                        matches!(state, OrderState::Cancelled | OrderState::Expired)
                    })
        })
        .await;
    match closed.and_then(|event| event.tag("s").and_then(OrderState::from_protocol_status)) {
        Some(state) => {
            ctx.timeline.push(
                mortsom_scenario_kit::timeline::TimelineKind::Assertion,
                None,
                format!(
                    "order {}: the maker's app no longer lists it; the relay shows it {state}",
                    order.id
                ),
                None,
            );
            Ok(state)
        }
        None => Err(format!(
            "{unread}; the maker's app no longer lists order {} and the relay never showed it \
             cancelled or expired",
            order.id
        )),
    }
}
