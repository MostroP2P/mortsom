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
        if *incoming && matches!(state, OrderState::Cancelled | OrderState::Expired) {
            node.cancel_invoice(hash).await?;
        }
        let payment = ctx
            .wallets
            .wait_terminal(wallet, order_id, hash, ctx.settle_timeout)
            .await
            .map_err(|e| e.to_string())?;
        let count = node.settled_record_count(hash, *incoming).await?;
        let safe = if state == OrderState::Completed {
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
            format!("wallet {wallet} order {order_id} hash {hash}: incoming={incoming}, principal={} sats, fee={} sats, settled ledger records={count}", payment.amount_sats, payment.fee_sats),
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
    let state = ctx
        .adapter
        .observe_order(&session.device(ctx.ui_timeout), &order)
        .await
        .map_err(|e| e.to_string())?
        .state;
    if matches!(state, OrderState::Completed | OrderState::PayoutPending) {
        for actor in Actor::ALL {
            mortsom_scenario_kit::actions::wait_app_state(
                ctx,
                actor,
                &order,
                OrderState::Completed,
            )
            .await
            .map_err(|e| e.to_string())?;
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
