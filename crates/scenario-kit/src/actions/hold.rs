//! Seller escrow funding and terminal budget accounting.
use super::*;
/// Pays the hold invoice as `actor` through the app's wallet path, within
/// the caps.
///
/// On the NWC path the wallet must report the payment settled. On the node
/// path the daemon holds the HTLC until release, so this only confirms the
/// payment is in flight; settlement is committed by
/// [`settle_hold_payment`] after release (or the refund observed by
/// [`expect_hold_refunded`] after a cancellation).
pub async fn pay_hold_invoice(
    ctx: &Context,
    actor: Actor,
    order: &OrderReference,
) -> Result<PaymentObservation, ScenarioError> {
    ctx.require_mainnet()?;
    if ctx.journal.enabled() && ctx.node_wallet(actor).is_none() {
        return Err(ScenarioError::environment(
            "durable desktop recovery requires a caller-controlled node wallet before any payment",
        ));
    }
    let session = ctx.actor(actor)?;
    let mut prepared = hold_budget::PreparedReservation::reserve(ctx, &order.id, ctx.amount_sats)?;
    ctx.timeline.push(
        TimelineKind::Step,
        Some(actor.as_str()),
        format!("reserved {} sats for order {}", ctx.amount_sats, order.id),
        None,
    );
    // With a node of our own the app has no wallet to pay with: it shows
    // the invoice and waits, and we pay it. Otherwise the app pays.
    let node = ctx.node_wallet(actor);
    let path = if node.is_some() {
        PaymentPath::ExternalWallet
    } else {
        PaymentPath::Nwc
    };
    if node.is_none() {
        // On NWC the app may initiate payment during this UI call. An error or
        // cancellation cannot prove that nothing moved, so retain the budget.
        prepared.payment_may_start(None)?;
    }
    let handoff = ctx
        .adapter
        .pay_hold_invoice(&session.device(ctx.ui_timeout), order, path)
        .await?;
    if handoff.kind != path {
        prepared.payment_may_start(None)?;
        return Err(ScenarioError::product(
            "hold invoice used a different payment path than requested",
        ));
    }
    let Some(invoice) = handoff.invoice.as_ref() else {
        return Err(ScenarioError::product(
            "the app did not expose the hold invoice; the payment cannot be correlated",
        ));
    };
    ctx.protect(invoice);
    let hash = payment_hash_of(invoice).map_err(|e| ScenarioError::product(e.to_string()))?;
    let amount_sats = if ctx.journal.enabled() {
        prepared.requote(invoice, node.map_or(0, |node| node.routing_fee_cap_sats()))?
    } else {
        ctx.amount_sats
    };
    ctx.journal
        .record(RecoveryEvent::PaymentExpected {
            order_id: order.id.clone(),
            wallet: actor.as_str().into(),
            hash: hash.clone(),
            incoming: false,
            amount_sats,
        })
        .map_err(ScenarioError::environment)?;
    prepared.payment_may_start(Some(hash.clone()))?;
    if let Some(node) = node {
        node.start_payment(invoice)
            .await
            .map_err(ScenarioError::environment)?;
    }
    let reservation = prepared.into_reservation();
    let step = ctx
        .evidence
        .checkpoint(actor.as_str(), "pay-hold-invoice", session.ui.as_ref())
        .await;
    ctx.timeline.push(
        TimelineKind::Step,
        Some(actor.as_str()),
        format!(
            "paid hold invoice for order {} via {:?} (hash {})",
            order.id,
            handoff.kind,
            short(&hash)
        ),
        Some(step),
    );
    if node.is_some() {
        return accept_hold(ctx, actor, order, &session.wallet_label, reservation, hash).await;
    }
    let observation = ctx
        .wallets
        .wait_terminal(&session.wallet_label, &order.id, &hash, ctx.settle_timeout)
        .await;
    match observation {
        Ok(obs) if obs.state == PaymentState::Settled => {
            record_settled(ctx, actor, order, &reservation, obs, &hash)
        }
        Ok(obs) => {
            release_quietly(ctx, &reservation);
            Err(ScenarioError::wallet(format!(
                "hold invoice payment ended {:?} for order {}",
                obs.state, order.id
            )))
        }
        Err(e) => {
            mark_in_flight(ctx, &reservation, Some(hash));
            Err(ScenarioError::wallet(e.to_string()))
        }
    }
}

/// Accepts a hold payment that is in flight from Mortsom's own node.
///
/// A Mostro hold invoice cannot settle before release: the daemon holds
/// the HTLC across fiat-sent and release, several steps away, and those
/// steps only run after this action returns. Requiring settlement here
/// would deadlock the trade, so the pending state is the success: the
/// reservation stays in flight and the hold is recorded for
/// [`settle_hold_payment`] or [`expect_hold_refunded`] to resolve.
async fn accept_hold(
    ctx: &Context,
    actor: Actor,
    order: &OrderReference,
    wallet_label: &str,
    reservation: crate::budget::Reservation,
    hash: String,
) -> Result<PaymentObservation, ScenarioError> {
    let observation = ctx
        .wallets
        .wait_known(wallet_label, &order.id, &hash, ctx.settle_timeout)
        .await;
    match observation {
        Ok(obs) if obs.state == PaymentState::Settled => {
            record_settled(ctx, actor, order, &reservation, obs, &hash)
        }
        Ok(obs) if obs.state == PaymentState::Failed => {
            release_quietly(ctx, &reservation);
            Err(ScenarioError::wallet(format!(
                "hold invoice payment failed for order {}",
                order.id
            )))
        }
        Ok(obs) => {
            mark_in_flight(ctx, &reservation, Some(hash.clone()));
            ctx.record_pending_hold(
                &order.id,
                PendingHold {
                    actor,
                    wallet_label: wallet_label.to_owned(),
                    payment_hash: hash.clone(),
                    reservation,
                },
            );
            ctx.timeline.push_at(
                obs.observed_at * 1000,
                TimelineKind::Wallet,
                Some(actor.as_str()),
                format!(
                    "wallet: hold invoice accepted for order {} ({} sats in flight; settlement follows release)",
                    order.id, obs.amount_sats
                ),
                Some(hash),
            );
            Ok(obs)
        }
        Err(e) => {
            mark_in_flight(ctx, &reservation, Some(hash));
            Err(ScenarioError::wallet(e.to_string()))
        }
    }
}

/// Records a settled payment: commits it against the budget and puts the
/// settlement on the timeline. A refused commit keeps the entry in flight
/// (the sats already left the wallet) and propagates the refusal.
fn record_settled(
    ctx: &Context,
    actor: Actor,
    order: &OrderReference,
    reservation: &crate::budget::Reservation,
    obs: PaymentObservation,
    hash: &str,
) -> Result<PaymentObservation, ScenarioError> {
    let mut budget = ctx.budget.lock().unwrap_or_else(|p| p.into_inner());
    let committed = commit_settlement(&mut budget, reservation, &obs, hash);
    drop(budget);
    if let Err(e) = committed {
        ctx.timeline.push(
            TimelineKind::Wallet,
            Some(actor.as_str()),
            format!(
                "settled {} sats (fee {}) for order {} but the budget refused it: {e}",
                obs.amount_sats, obs.fee_sats, order.id
            ),
            Some(hash.to_owned()),
        );
        return Err(ScenarioError::environment(e.to_string()));
    }
    ctx.timeline.push_at(
        obs.observed_at * 1000,
        TimelineKind::Wallet,
        Some(actor.as_str()),
        format!(
            "wallet: {} sats settled (fee {}) for order {}",
            obs.amount_sats, obs.fee_sats, order.id
        ),
        Some(hash.to_owned()),
    );
    Ok(obs)
}

/// Settlement of the held HTLC, after release. On the node path funding
/// only proved the payment was in flight; here the seller's wallet must
/// show it settled and the budget records the spend, fee included. No-op
/// when the order has no pending hold (the NWC path settles at funding).
pub async fn settle_hold_payment(
    ctx: &Context,
    order: &OrderReference,
) -> Result<Option<PaymentObservation>, ScenarioError> {
    let Some(pending) = pending_snapshot(ctx, &order.id) else {
        return Ok(None);
    };
    let obs = ctx
        .wallets
        .wait_terminal(
            &pending.wallet_label,
            &order.id,
            &pending.payment_hash,
            ctx.settle_timeout,
        )
        .await
        // The reservation stays in flight: the HTLC's fate is unknown.
        .map_err(|e| ScenarioError::wallet(e.to_string()))?;
    if obs.state != PaymentState::Settled {
        return Err(ScenarioError::wallet(format!(
            "hold payment for order {} ended {:?} after release",
            order.id, obs.state
        )));
    }
    let settled = record_settled(
        ctx,
        pending.actor,
        order,
        &pending.reservation,
        obs,
        &pending.payment_hash,
    )?;
    remove_resolved_pending(ctx, &order.id, pending.reservation.id())?;
    Ok(Some(settled))
}

/// Refund of the held HTLC, after a cancellation: the daemon returns the
/// escrow, so the payment must fail and the reservation is freed. No-op
/// when the order has no pending hold.
pub async fn expect_hold_refunded(
    ctx: &Context,
    order: &OrderReference,
) -> Result<(), ScenarioError> {
    let Some(pending) = pending_snapshot(ctx, &order.id) else {
        return Ok(());
    };
    let obs = ctx
        .wallets
        .wait_terminal(
            &pending.wallet_label,
            &order.id,
            &pending.payment_hash,
            ctx.settle_timeout,
        )
        .await
        .map_err(|e| ScenarioError::wallet(e.to_string()))?;
    if obs.state == PaymentState::Failed {
        ctx.budget
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .release(&pending.reservation)
            .map_err(|error| ScenarioError::environment(error.to_string()))?;
        remove_resolved_pending(ctx, &order.id, pending.reservation.id())?;
        ctx.timeline.push_at(
            obs.observed_at * 1000,
            TimelineKind::Wallet,
            Some(pending.actor.as_str()),
            format!(
                "wallet: hold payment for cancelled order {} returned",
                order.id
            ),
            Some(pending.payment_hash.clone()),
        );
        return Ok(());
    }
    // Sats left the wallet on a cancelled order: keep the accounting
    // honest, then report the product failure.
    let state = obs.state;
    record_settled(
        ctx,
        pending.actor,
        order,
        &pending.reservation,
        obs,
        &pending.payment_hash,
    )?;
    remove_resolved_pending(ctx, &order.id, pending.reservation.id())?;
    Err(ScenarioError::product(format!(
        "hold payment for cancelled order {} ended {state:?}; the escrow was not returned",
        order.id
    )))
}

// Keep the authoritative pending handle in Context across every wallet await.
// A timeout, cancellation or accounting refusal leaves it available to retry.
fn pending_snapshot(ctx: &Context, order_id: &str) -> Option<PendingHold> {
    ctx.pending_holds
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .get(order_id)
        .map(|pending| PendingHold {
            actor: pending.actor,
            wallet_label: pending.wallet_label.clone(),
            payment_hash: pending.payment_hash.clone(),
            reservation: pending.reservation.clone(),
        })
}

fn remove_resolved_pending(
    ctx: &Context,
    order_id: &str,
    reservation: u64,
) -> Result<(), ScenarioError> {
    let mut pending = ctx.pending_holds.lock().unwrap_or_else(|p| p.into_inner());
    if !pending
        .get(order_id)
        .is_some_and(|hold| hold.reservation.id() == reservation)
    {
        return Err(ScenarioError::environment(
            "pending hold ownership changed during resolution",
        ));
    }
    pending.remove(order_id);
    Ok(())
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod ownership_tests {
    use super::*;
    use crate::test_support::{StateAdapter, context};
    use mortsom_observers::wallet::{WalletObserver, WalletReader};
    use std::sync::{Arc, Mutex};

    struct Reader(Mutex<PaymentState>);
    #[async_trait::async_trait]
    impl WalletReader for Reader {
        async fn lookup(&self, _: &str) -> Result<Option<(u64, u64, PaymentState)>, String> {
            let state = *self.0.lock().unwrap();
            if state == PaymentState::Unknown {
                std::future::pending::<()>().await;
            }
            Ok(Some((1, 0, state)))
        }
    }

    fn pending() -> (Context, Arc<Reader>) {
        let mut ctx = context(Arc::new(StateAdapter(Mutex::new(OrderState::Completed))));
        let reader = Arc::new(Reader(Mutex::new(PaymentState::Pending)));
        ctx.wallets = Arc::new(
            WalletObserver::new(vec![("alice".into(), reader.clone())])
                .with_poll(Duration::from_millis(1)),
        );
        ctx.settle_timeout = Duration::from_millis(5);
        let reservation = ctx.budget.lock().unwrap().reserve("o1", 1).unwrap();
        ctx.budget
            .lock()
            .unwrap()
            .mark_in_flight(&reservation, Some("hash".into()))
            .unwrap();
        ctx.record_pending_hold(
            "o1",
            PendingHold {
                actor: Actor::Alice,
                wallet_label: "alice".into(),
                payment_hash: "hash".into(),
                reservation,
            },
        );
        (ctx, reader)
    }

    #[tokio::test]
    async fn settlement_and_refund_timeouts_retain_the_pending_hold_until_a_retry_resolves_it() {
        let order = OrderReference::new("o1");
        for refund in [false, true] {
            let (ctx, reader) = pending();
            let failed = if refund {
                expect_hold_refunded(&ctx, &order).await
            } else {
                settle_hold_payment(&ctx, &order).await.map(|_| ())
            };
            assert!(failed.is_err());
            assert!(ctx.pending_holds.lock().unwrap().contains_key("o1"));
            *reader.0.lock().unwrap() = if refund {
                PaymentState::Failed
            } else {
                PaymentState::Settled
            };
            if refund {
                expect_hold_refunded(&ctx, &order).await.unwrap();
            } else {
                assert!(settle_hold_payment(&ctx, &order).await.unwrap().is_some());
            }
            assert!(!ctx.pending_holds.lock().unwrap().contains_key("o1"));
            assert!(!ctx.budget.lock().unwrap().has_open_reservations());
        }
    }

    #[tokio::test]
    async fn cancellation_while_the_wallet_is_unresponsive_keeps_the_pending_handle() {
        let (ctx, reader) = pending();
        *reader.0.lock().unwrap() = PaymentState::Unknown;
        assert!(
            tokio::time::timeout(
                Duration::from_millis(5),
                settle_hold_payment(&ctx, &OrderReference::new("o1"))
            )
            .await
            .is_err()
        );
        assert!(ctx.pending_holds.lock().unwrap().contains_key("o1"));
        assert!(ctx.budget.lock().unwrap().has_open_reservations());
    }
}
