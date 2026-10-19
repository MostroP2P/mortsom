//! Correlates terminal public UI states with exact run-owned Lightning records.
use super::*;
use mortsom_app_contract::{OrderReference, OrderState};
use mortsom_observers::wallet::PaymentState;

pub(in crate::run) async fn verify_payments(
    ctx: &Context,
    journal: &FileJournal,
) -> Result<(), String> {
    let events = journal.snapshot().events;
    // A terminal state is final: read once, it holds for every payment on
    // the order. Each payment is still settled by its own hash below.
    let mut terminal: BTreeMap<&str, OrderState> = BTreeMap::new();
    for event in &events {
        let RecoveryEvent::PaymentExpected {
            order_id,
            wallet,
            hash,
            incoming,
            amount_sats,
            bond,
        } = event
        else {
            continue;
        };
        let state = match terminal.get(order_id.as_str()) {
            Some(state) => *state,
            None => {
                let state = terminal_order_state(ctx, &events, order_id).await?;
                terminal.insert(order_id, state);
                state
            }
        };
        let node = ctx
            .node_wallets
            .get(wallet)
            .ok_or("payment recovery requires the original node wallet")?;
        // A payment the scenario withdrew on purpose (a refused or replaced
        // payout invoice) must never have settled, whatever the order came
        // to; the node is asked to cancel it again, which is idempotent.
        let withdrawn = *incoming && crate::recovery_store::superseded(&events, wallet, hash);
        // A bond's share is paid on a cancelled order, so it is never
        // cancelled here; the exact settlement is checked below.
        if withdrawn
            || (*incoming && !*bond && matches!(state, OrderState::Cancelled | OrderState::Expired))
        {
            node.cancel_invoice(hash).await?;
        }
        let payment = ctx
            .wallets
            .wait_terminal(wallet, order_id, hash, ctx.settle_timeout)
            .await
            .map_err(|e| e.to_string())?;
        let count = node.settled_record_count(hash, *incoming).await?;
        // A bond ends one of two ways whatever the order came to: returned
        // (the daemon cancelled the HTLC it held) or slashed (settled, once,
        // for exactly the bond). Which one the scenario asserted itself;
        // what recovery refuses is a bond still in flight.
        let safe = if withdrawn {
            payment.state == PaymentState::Failed && count == 0
        } else if *bond {
            // A claim is received whole; a slashed bond paid at most the fee
            // ceiling on top.
            let fee_ok = if *incoming {
                payment.fee_sats == 0
            } else {
                payment.fee_sats <= node.routing_fee_cap_sats()
            };
            fee_ok
                && if *incoming {
                    claim_resolved(payment.state, payment.amount_sats, *amount_sats, count)
                } else {
                    bond_resolved(payment.state, payment.amount_sats, *amount_sats, count)
                }
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
    let state = match ctx.adapter.observe_listed_order(&device, &order).await {
        Ok(Some(shown)) => shown.state,
        Ok(None) => return closed_off_the_list(ctx, &order).await,
        Err(unread) => return Err(unread.to_string()),
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

/// The terminal state of an order the maker's loaded list no longer shows.
/// App v2 drops a trade that never went active from My Trades once it is
/// cancelled, so its state goes with the row: the daemon's own status on
/// the relay decides, and only a cancel or an expiry can, since a completed
/// trade stays listed.
async fn closed_off_the_list(ctx: &Context, order: &OrderReference) -> Result<OrderState, String> {
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
            "the maker's app no longer lists order {} and the relay never showed it cancelled or \
             expired",
            order.id
        )),
    }
}

/// Whether a bond's payment record is at rest: returned (failed, nothing
/// settled) or slashed (settled once for exactly the bond).
fn bond_resolved(
    state: PaymentState,
    paid_sats: u64,
    expected_sats: u64,
    settled_records: usize,
) -> bool {
    match state {
        PaymentState::Settled => paid_sats == expected_sats && settled_records == 1,
        PaymentState::Failed => settled_records == 0,
        _ => false,
    }
}

/// Whether a claim of a slashed bond's share is at rest: paid, once, for
/// exactly the share. A claim invoice that failed without the scenario
/// withdrawing it (that case is judged before this) leaves the daemon
/// owing the share and asking for a replacement, which only the retained
/// app can hand in.
fn claim_resolved(
    state: PaymentState,
    paid_sats: u64,
    expected_sats: u64,
    settled_records: usize,
) -> bool {
    state == PaymentState::Settled && paid_sats == expected_sats && settled_records == 1
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_returned_bond_is_resolved_without_a_settled_record() {
        assert!(bond_resolved(PaymentState::Failed, 0, 2500, 0));
        assert!(!bond_resolved(PaymentState::Failed, 0, 2500, 1));
    }

    #[test]
    fn a_slashed_bond_is_resolved_when_settled_once_for_the_bond() {
        assert!(bond_resolved(PaymentState::Settled, 2500, 2500, 1));
        assert!(!bond_resolved(PaymentState::Settled, 2400, 2500, 1));
        assert!(!bond_resolved(PaymentState::Settled, 2500, 2500, 2));
    }

    #[test]
    fn a_bond_still_in_flight_is_not_resolved() {
        assert!(!bond_resolved(PaymentState::Pending, 2500, 2500, 0));
    }

    /// A claim invoice that failed without the scenario withdrawing it
    /// leaves the daemon owing the share: it asks for a replacement, which
    /// only the retained app can hand in, so recovery must not close it.
    #[test]
    fn a_failed_claim_never_withdrawn_is_not_resolved() {
        assert!(!claim_resolved(PaymentState::Failed, 0, 500, 0));
        assert!(claim_resolved(PaymentState::Settled, 500, 500, 1));
        assert!(!claim_resolved(PaymentState::Settled, 500, 500, 2));
    }
}
