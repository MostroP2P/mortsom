//! Domain actions (spec §7): each finishes only after its expected visible
//! state appears and, when appropriate, its external consequence is
//! observed. Every action records evidence and a timeline entry.

use std::fmt;
use std::sync::Mutex;
use std::time::Duration;

use mortsom_android_driver::avd::Actor;
use mortsom_app_contract::{OrderDraft, OrderReference, OrderState, PaymentPath};
use mortsom_observers::wallet::{PaymentObservation, PaymentState, payment_hash_of};
use tracing::info;

use crate::context::{Context, PendingHold, ScenarioError};
use crate::timeline::TimelineKind;
use crate::waits::{stays_absent, wait_until};

/// Poll interval for state observations.
pub const STATE_POLL: Duration = Duration::from_secs(3);
/// Wire kind of order events.
const ORDER_EVENT_KIND: u16 = 38383;

/// Creates an order as `actor` and registers its cleanup immediately.
pub async fn create_order(
    ctx: &Context,
    actor: Actor,
    draft: &OrderDraft,
) -> Result<OrderReference, ScenarioError> {
    let session = ctx.actor(actor)?;
    let visible = ctx
        .adapter
        .create_order(&session.device(ctx.ui_timeout), draft)
        .await?;
    let id = visible
        .id
        .ok_or_else(|| ScenarioError::product("the app shows no id for the created order"))?;
    let order = OrderReference::with_kind(id.clone(), draft.kind);
    ctx.cleanup.register(actor, &order);
    ctx.relay
        .add_order(&id)
        .await
        .map_err(|e| ScenarioError::relay(format!("cannot scope observer to order {id}: {e}")))?;
    let step = ctx
        .evidence
        .checkpoint(actor.as_str(), "create-order", session.ui.as_ref())
        .await;
    ctx.timeline.push(
        TimelineKind::Step,
        Some(actor.as_str()),
        format!(
            "created {:?} order {id} ({} sats, {} {})",
            draft.kind, draft.amount_sats, draft.fiat_amount, draft.fiat_code
        ),
        Some(step),
    );
    info!(actor = %actor, order = %id, "order created");
    Ok(order)
}

/// Takes `order` as `actor`.
pub async fn take_order(
    ctx: &Context,
    actor: Actor,
    order: &OrderReference,
) -> Result<(), ScenarioError> {
    let session = ctx.actor(actor)?;
    ctx.adapter
        .find_and_take_order(&session.device(ctx.ui_timeout), order)
        .await?;
    let step = ctx
        .evidence
        .checkpoint(actor.as_str(), "take-order", session.ui.as_ref())
        .await;
    ctx.timeline.push(
        TimelineKind::Step,
        Some(actor.as_str()),
        format!("took order {}", order.id),
        Some(step),
    );
    Ok(())
}

/// Waits until `actor`'s app shows `expected` for `order`, bounded by the
/// state timeout. Returns the raw label the app rendered.
pub async fn wait_app_state(
    ctx: &Context,
    actor: Actor,
    order: &OrderReference,
    expected: OrderState,
) -> Result<String, ScenarioError> {
    let session = ctx.actor(actor)?;
    let device = session.device(ctx.ui_timeout);
    let last: Mutex<Option<LastSeen>> = Mutex::new(None);
    let found = wait_until(ctx.state_timeout, STATE_POLL, || async {
        match ctx.adapter.observe_order(&device, order).await {
            Ok(state) if state.state == expected => Some(Ok(state.raw_label)),
            // A terminal state other than the expected one can never become
            // it: fail fast instead of waiting out the deadline.
            Ok(state) if state.state.is_terminal() => Some(Err(ScenarioError::product(format!(
                "order {} reached terminal `{}` while waiting for {expected}",
                order.id, state.raw_label
            )))),
            Ok(state) => {
                *last.lock().unwrap_or_else(|p| p.into_inner()) =
                    Some(LastSeen::Status(state.raw_label));
                None
            }
            // An observation that failed is not the app showing the wrong
            // state: it is kept, so the deadline can say which it was.
            Err(e) => {
                *last.lock().unwrap_or_else(|p| p.into_inner()) =
                    Some(LastSeen::Unobserved(e.to_string()));
                None
            }
        }
    })
    .await;
    match found {
        Some(Ok(raw)) => {
            let step = ctx
                .evidence
                .checkpoint(
                    actor.as_str(),
                    &format!("state-{expected}"),
                    session.ui.as_ref(),
                )
                .await;
            ctx.timeline.push(
                TimelineKind::UiState,
                Some(actor.as_str()),
                format!("order {} shows {expected} (`{raw}`)", order.id),
                Some(step),
            );
            Ok(raw)
        }
        Some(Err(e)) => Err(e),
        None => {
            ctx.evidence
                .checkpoint(
                    actor.as_str(),
                    &format!("state-{expected}-timeout"),
                    session.ui.as_ref(),
                )
                .await;
            Err(ScenarioError::product(state_timeout_message(
                order,
                &expected.to_string(),
                actor,
                ctx.state_timeout,
                last.lock().unwrap_or_else(|p| p.into_inner()).as_ref(),
            )))
        }
    }
}

/// Waits (bounded by `timeout`) until `actor`'s app shows `order` in one of
/// `accepted` and returns which. Any other terminal state fails fast.
pub async fn wait_app_state_in(
    ctx: &Context,
    actor: Actor,
    order: &OrderReference,
    accepted: &[OrderState],
    timeout: Duration,
) -> Result<OrderState, ScenarioError> {
    let session = ctx.actor(actor)?;
    let device = session.device(ctx.ui_timeout);
    let found = wait_until(timeout, STATE_POLL, || async {
        match ctx.adapter.observe_order(&device, order).await {
            Ok(state) if accepted.contains(&state.state) => Some(Ok(state)),
            Ok(state) if state.state.is_terminal() => Some(Err(ScenarioError::product(format!(
                "order {} reached terminal `{}` while waiting for {}",
                order.id,
                state.raw_label,
                names(accepted)
            )))),
            _ => None,
        }
    })
    .await;
    match found {
        Some(Ok(state)) => {
            ctx.timeline.push(
                TimelineKind::UiState,
                Some(actor.as_str()),
                format!(
                    "order {} shows {} (`{}`)",
                    order.id, state.state, state.raw_label
                ),
                None,
            );
            Ok(state.state)
        }
        Some(Err(e)) => Err(e),
        None => Err(ScenarioError::product(format!(
            "order {} did not reach {} on {actor} within {timeout:?}",
            order.id,
            names(accepted)
        ))),
    }
}

/// What the last completed observation of an order said.
///
/// A wait that ends without its state has to say which of the two happened:
/// the app showed something else, or the app was never read at all. They
/// point at opposite halves of the system, and a message that omits it
/// blames the product for what may be a broken navigation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LastSeen {
    /// The app rendered this raw status label.
    Status(String),
    /// The observation itself failed, for this reason.
    Unobserved(String),
}

impl fmt::Display for LastSeen {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LastSeen::Status(raw) => write!(f, "last showed `{raw}`"),
            LastSeen::Unobserved(why) => write!(f, "was never read: {why}"),
        }
    }
}

/// The message a state wait leaves when its deadline passes.
fn state_timeout_message(
    order: &OrderReference,
    expected: &str,
    actor: Actor,
    timeout: Duration,
    last: Option<&LastSeen>,
) -> String {
    let tail = match last {
        Some(seen) => format!("; the app {seen}"),
        None => String::from("; the app was never read"),
    };
    format!(
        "order {} did not reach {expected} on {actor} within {timeout:?}{tail}",
        order.id
    )
}

fn names(states: &[OrderState]) -> String {
    states
        .iter()
        .map(|s| s.as_str())
        .collect::<Vec<_>>()
        .join("|")
}

/// Negative observation: `actor`'s app keeps showing `order` in `expected`
/// for the whole `window`; any other state within it is a product failure.
pub async fn expect_state_holds(
    ctx: &Context,
    actor: Actor,
    order: &OrderReference,
    expected: OrderState,
    window: Duration,
) -> Result<(), ScenarioError> {
    let session = ctx.actor(actor)?;
    let device = session.device(ctx.ui_timeout);
    let deviation = stays_absent(window, STATE_POLL, || async {
        match ctx.adapter.observe_order(&device, order).await {
            Ok(state) if state.state != expected => Some(state.raw_label),
            _ => None,
        }
    })
    .await;
    match deviation {
        None => {
            ctx.timeline.push(
                TimelineKind::Assertion,
                Some(actor.as_str()),
                format!("order {} stayed {expected} for {window:?}", order.id),
                None,
            );
            Ok(())
        }
        Some(raw) => Err(ScenarioError::product(format!(
            "order {} left {expected} (`{raw}`) on {actor} within {window:?}",
            order.id
        ))),
    }
}

/// Waits until both apps show `expected`.
pub async fn wait_both(
    ctx: &Context,
    order: &OrderReference,
    expected: OrderState,
) -> Result<(), ScenarioError> {
    for actor in Actor::ALL {
        wait_app_state(ctx, actor, order, expected).await?;
    }
    Ok(())
}

/// Correlation (spec §8): the run relay must show `order` in `expected`.
pub async fn wait_relay_state(
    ctx: &Context,
    order: &OrderReference,
    expected: OrderState,
) -> Result<(), ScenarioError> {
    let id = order.id.clone();
    let event = ctx
        .relay
        .wait_for(ctx.state_timeout, |e| {
            e.kind == ORDER_EVENT_KIND
                && e.tag("d") == Some(id.as_str())
                && e.tag("s").and_then(OrderState::from_protocol_status) == Some(expected)
        })
        .await
        .ok_or_else(|| {
            ScenarioError::relay(format!(
                "no relay event shows order {} as {expected} within {:?}",
                order.id, ctx.state_timeout
            ))
        })?;
    ctx.timeline.push_at(
        event.received_at_ms,
        TimelineKind::RelayEvent,
        None,
        format!(
            "relay: order {} is {expected} (event {})",
            order.id,
            short(&event.id)
        ),
        Some(event.id.clone()),
    );
    Ok(())
}

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
    let session = ctx.actor(actor)?;
    let reservation = {
        let mut budget = ctx.budget.lock().unwrap_or_else(|p| p.into_inner());
        budget
            .reserve(&order.id, ctx.amount_sats)
            .map_err(|e| ScenarioError::environment(e.to_string()))?
    };
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
    let handoff = match ctx
        .adapter
        .pay_hold_invoice(&session.device(ctx.ui_timeout), order, path)
        .await
    {
        Ok(handoff) => handoff,
        Err(e) => {
            release_quietly(ctx, &reservation);
            return Err(e.into());
        }
    };
    let Some(invoice) = handoff.invoice.as_ref() else {
        mark_in_flight(ctx, &reservation, None);
        return Err(ScenarioError::product(
            "the app did not expose the hold invoice; the payment cannot be correlated",
        ));
    };
    ctx.protect(invoice);
    let hash = payment_hash_of(invoice).map_err(|e| ScenarioError::product(e.to_string()))?;
    if let Some(node) = node
        && let Err(e) = node.start_payment(invoice).await
    {
        mark_in_flight(ctx, &reservation, Some(hash.clone()));
        return Err(ScenarioError::environment(e));
    }
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
    let Some(pending) = ctx.take_pending_hold(&order.id) else {
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
    record_settled(
        ctx,
        pending.actor,
        order,
        &pending.reservation,
        obs,
        &pending.payment_hash,
    )
    .map(Some)
}

/// Refund of the held HTLC, after a cancellation: the daemon returns the
/// escrow, so the payment must fail and the reservation is freed. No-op
/// when the order has no pending hold.
pub async fn expect_hold_refunded(
    ctx: &Context,
    order: &OrderReference,
) -> Result<(), ScenarioError> {
    let Some(pending) = ctx.take_pending_hold(&order.id) else {
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
        release_quietly(ctx, &pending.reservation);
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
    let _ = record_settled(
        ctx,
        pending.actor,
        order,
        &pending.reservation,
        obs,
        &pending.payment_hash,
    );
    Err(ScenarioError::product(format!(
        "hold payment for cancelled order {} ended {state:?}; the escrow was not returned",
        order.id
    )))
}

/// Generates the buyer invoice as `actor` (no value moves).
pub async fn provide_buyer_invoice(
    ctx: &Context,
    actor: Actor,
    order: &OrderReference,
) -> Result<Option<String>, ScenarioError> {
    let session = ctx.actor(actor)?;
    // With a node of our own the app has no wallet to issue from, so the
    // invoice is ours and the app only has to accept it.
    let supplied = match ctx.node_wallet(actor) {
        Some(node) => Some(
            node.make_invoice(ctx.amount_sats, &format!("mortsom {}", order.id))
                .await
                .map_err(ScenarioError::environment)?,
        ),
        None => None,
    };
    if let Some(invoice) = supplied.as_ref() {
        ctx.protect(invoice);
    }
    let invoice = ctx
        .adapter
        .provide_buyer_invoice(&session.device(ctx.ui_timeout), order, supplied.as_ref())
        .await?;
    let hash = match &invoice {
        Some(secret) => {
            ctx.protect(secret);
            Some(payment_hash_of(secret).map_err(|e| ScenarioError::product(e.to_string()))?)
        }
        None => None,
    };
    let step = ctx
        .evidence
        .checkpoint(actor.as_str(), "buyer-invoice", session.ui.as_ref())
        .await;
    ctx.timeline.push(
        TimelineKind::Step,
        Some(actor.as_str()),
        format!(
            "provided buyer invoice for order {} (hash {})",
            order.id,
            hash.as_deref()
                .map(short)
                .unwrap_or_else(|| "unknown".into())
        ),
        Some(step),
    );
    Ok(hash)
}

/// Reports fiat sent as `actor`.
pub async fn confirm_fiat_sent(
    ctx: &Context,
    actor: Actor,
    order: &OrderReference,
) -> Result<(), ScenarioError> {
    let session = ctx.actor(actor)?;
    ctx.adapter
        .confirm_fiat_sent(&session.device(ctx.ui_timeout), order)
        .await?;
    let step = ctx
        .evidence
        .checkpoint(actor.as_str(), "fiat-sent", session.ui.as_ref())
        .await;
    ctx.timeline.push(
        TimelineKind::Step,
        Some(actor.as_str()),
        format!("reported fiat sent for order {}", order.id),
        Some(step),
    );
    Ok(())
}

/// Releases as `actor`.
pub async fn release(
    ctx: &Context,
    actor: Actor,
    order: &OrderReference,
) -> Result<(), ScenarioError> {
    let session = ctx.actor(actor)?;
    ctx.adapter
        .release(&session.device(ctx.ui_timeout), order)
        .await?;
    let step = ctx
        .evidence
        .checkpoint(actor.as_str(), "release", session.ui.as_ref())
        .await;
    ctx.timeline.push(
        TimelineKind::Step,
        Some(actor.as_str()),
        format!("released order {}", order.id),
        Some(step),
    );
    Ok(())
}

/// Settlement correlation: the buyer's wallet must show the payout for
/// `hash` settled.
pub async fn expect_payout_settled(
    ctx: &Context,
    actor: Actor,
    order: &OrderReference,
    hash: &str,
) -> Result<PaymentObservation, ScenarioError> {
    let session = ctx.actor(actor)?;
    let obs = ctx
        .wallets
        .wait_terminal(&session.wallet_label, &order.id, hash, ctx.settle_timeout)
        .await
        .map_err(|e| ScenarioError::wallet(e.to_string()))?;
    if obs.state != PaymentState::Settled {
        return Err(ScenarioError::wallet(format!(
            "payout {} ended {:?}",
            short(hash),
            obs.state
        )));
    }
    ctx.timeline.push_at(
        obs.observed_at * 1000,
        TimelineKind::Wallet,
        Some(actor.as_str()),
        format!(
            "wallet: payout {} sats received for order {}",
            obs.amount_sats, order.id
        ),
        Some(hash.to_owned()),
    );
    Ok(obs)
}

/// Marks the order resolved so cleanup does not try to cancel it.
pub fn mark_resolved(ctx: &Context, order: &OrderReference) {
    ctx.cleanup.resolved(order);
}

/// Records a settled payment against its reservation. The sats already
/// left the wallet, so a refused commit (an overspend above the reserved
/// amount) must not leave the entry `Reserved`: it is kept as in flight,
/// which still counts towards the run spend, and the refusal propagates.
fn commit_settlement(
    budget: &mut crate::budget::Budget,
    reservation: &crate::budget::Reservation,
    observation: &mortsom_observers::wallet::PaymentObservation,
    hash: &str,
) -> Result<(), crate::budget::BudgetError> {
    let Err(refused) = budget.commit(
        reservation,
        observation.amount_sats,
        observation.fee_sats,
        Some(hash.to_owned()),
    ) else {
        return Ok(());
    };
    // A failure here means the reservation itself is unknown; the refusal
    // above is the one worth reporting.
    let _ = budget.mark_in_flight(reservation, Some(hash.to_owned()));
    Err(refused)
}

fn release_quietly(ctx: &Context, reservation: &crate::budget::Reservation) {
    let mut budget = ctx.budget.lock().unwrap_or_else(|p| p.into_inner());
    if let Err(e) = budget.release(reservation) {
        ctx.timeline.push(
            TimelineKind::Step,
            None,
            format!("budget release failed: {e}"),
            None,
        );
    }
}

fn mark_in_flight(ctx: &Context, reservation: &crate::budget::Reservation, hash: Option<String>) {
    let mut budget = ctx.budget.lock().unwrap_or_else(|p| p.into_inner());
    if let Err(e) = budget.mark_in_flight(reservation, hash) {
        ctx.timeline.push(
            TimelineKind::Step,
            None,
            format!("budget in-flight mark failed: {e}"),
            None,
        );
    }
}

fn short(id: &str) -> String {
    id.chars().take(12).collect()
}

/// Whether a payment path may be observed through the wallet at all.
///
/// Both paths Mortsom drives are observable, for the same reason: the
/// wallet that moved the money is one Mortsom can ask. What is not
/// observable is a payment handed to a wallet on the operator's phone —
/// that is [`PaymentPath::ExternalWallet`] with no node configured, and the
/// caller knows which case it is in because it is the one that chose.
pub fn is_observable(path: PaymentPath) -> bool {
    matches!(path, PaymentPath::Nwc | PaymentPath::ExternalWallet)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use std::sync::{Arc, Mutex};

    use super::*;
    use crate::test_support::{StateAdapter, context};

    #[tokio::test]
    async fn expect_state_holds_watches_the_whole_window() {
        let adapter = Arc::new(StateAdapter(Mutex::new(OrderState::Active)));
        let ctx = context(adapter.clone());
        let order = OrderReference::new("o1");
        expect_state_holds(
            &ctx,
            Actor::Bob,
            &order,
            OrderState::Active,
            Duration::from_millis(30),
        )
        .await
        .unwrap();

        let flipper = adapter.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(20)).await;
            *flipper.0.lock().unwrap() = OrderState::Cancelled;
        });
        let err = expect_state_holds(
            &ctx,
            Actor::Bob,
            &order,
            OrderState::Active,
            Duration::from_millis(500),
        )
        .await
        .unwrap_err();
        assert!(err.message.contains("left active (`cancelled`)"), "{err}");
    }

    #[tokio::test]
    async fn wait_app_state_fails_fast_on_a_foreign_terminal_state() {
        let adapter = Arc::new(StateAdapter(Mutex::new(OrderState::Cancelled)));
        let ctx = context(adapter);
        let order = OrderReference::new("o1");
        let started = std::time::Instant::now();

        let err = wait_app_state(&ctx, Actor::Alice, &order, OrderState::Active)
            .await
            .unwrap_err();

        assert!(err.message.contains("terminal `cancelled`"));
        assert!(
            started.elapsed() < Duration::from_millis(250),
            "must not wait out the deadline"
        );
    }

    #[tokio::test]
    async fn wait_app_state_keeps_waiting_through_earlier_states_and_times_out() {
        let adapter = Arc::new(StateAdapter(Mutex::new(OrderState::Pending)));
        let ctx = context(adapter.clone());
        let order = OrderReference::new("o1");
        let err = wait_app_state(&ctx, Actor::Alice, &order, OrderState::Taken)
            .await
            .unwrap_err();
        assert!(err.message.contains("did not reach taken"));
        assert!(
            err.message.contains("last showed `pending`"),
            "the deadline must name what the app actually showed: {err}"
        );

        *adapter.0.lock().unwrap() = OrderState::Taken;
        let raw = wait_app_state(&ctx, Actor::Alice, &order, OrderState::Taken)
            .await
            .unwrap();
        assert_eq!(raw, "taken");
    }

    /// A wait that ends without its state has to distinguish the app showing
    /// something else from the app never being read: the first is the
    /// product, the second is the harness, and a message that says neither
    /// sends the reader to the wrong half of the system.
    #[test]
    fn a_state_deadline_says_what_the_last_observation_was() {
        let order = OrderReference::new("o1");
        let timeout = Duration::from_secs(180);

        let showed = state_timeout_message(
            &order,
            "pending",
            Actor::Alice,
            timeout,
            Some(&LastSeen::Status("waiting-payment".into())),
        );
        assert!(showed.contains("last showed `waiting-payment`"), "{showed}");

        let blind = state_timeout_message(
            &order,
            "pending",
            Actor::Alice,
            timeout,
            Some(&LastSeen::Unobserved("trades.item.o1 not found".into())),
        );
        assert!(
            blind.contains("was never read: trades.item.o1 not found"),
            "{blind}"
        );
        assert!(
            !blind.contains("last showed"),
            "an unread app is not a state the app showed: {blind}"
        );

        let never = state_timeout_message(&order, "pending", Actor::Alice, timeout, None);
        assert!(never.contains("the app was never read"), "{never}");
    }

    #[tokio::test]
    async fn value_moving_actions_require_clearance_before_touching_the_wallet() {
        let ctx = context(Arc::new(StateAdapter(Mutex::new(OrderState::Taken))));
        let err = pay_hold_invoice(&ctx, Actor::Alice, &OrderReference::new("o1"))
            .await
            .unwrap_err();
        assert_eq!(
            err.category,
            mortsom_common::FailureCategory::EnvironmentMisconfiguration
        );
        assert!(
            ctx.budget.lock().unwrap().ledger().is_empty(),
            "nothing reserved without clearance"
        );
    }
    /// Without a pending hold (the NWC path) release and cancel have no
    /// deferred settlement to resolve; both must be clean no-ops.
    #[tokio::test]
    async fn hold_resolution_is_a_no_op_without_a_pending_hold() {
        let ctx = context(Arc::new(StateAdapter(Mutex::new(OrderState::Completed))));
        let order = OrderReference::new("o1");

        assert_eq!(settle_hold_payment(&ctx, &order).await.unwrap(), None);
        expect_hold_refunded(&ctx, &order).await.unwrap();
        assert!(ctx.budget.lock().unwrap().ledger().is_empty());
    }

    /// After release the pending hold must settle and its reservation is
    /// committed against the budget, fee included; after a cancellation
    /// the payment must fail and the reservation is freed.
    #[tokio::test]
    async fn a_pending_hold_settles_after_release_and_refunds_after_cancel() {
        use crate::budget::EntryStatus;
        use crate::context::PendingHold;

        let mut ctx = context(Arc::new(StateAdapter(Mutex::new(OrderState::Completed))));
        ctx.wallets = Arc::new(mortsom_observers::wallet::WalletObserver::new(vec![(
            "alice".into(),
            Arc::new(SettledWallet) as Arc<dyn mortsom_observers::wallet::WalletReader>,
        )]));
        let order = OrderReference::new("o1");
        let reservation = ctx.budget.lock().unwrap().reserve("o1", 5).unwrap();
        ctx.record_pending_hold(
            "o1",
            PendingHold {
                actor: Actor::Alice,
                wallet_label: "alice".into(),
                payment_hash: "aa".into(),
                reservation,
            },
        );

        let obs = settle_hold_payment(&ctx, &order).await.unwrap().unwrap();

        assert_eq!(obs.state, PaymentState::Settled);
        {
            let budget = ctx.budget.lock().unwrap();
            assert_eq!(budget.ledger()[0].status, EntryStatus::Settled);
            assert_eq!(budget.ledger()[0].fee_sats, 1);
        }

        // A second call finds nothing: the hold was consumed.
        assert_eq!(settle_hold_payment(&ctx, &order).await.unwrap(), None);

        // Cancellation path: the payment fails and the budget is freed.
        ctx.wallets = Arc::new(mortsom_observers::wallet::WalletObserver::new(vec![(
            "alice".into(),
            Arc::new(FailedWallet) as Arc<dyn mortsom_observers::wallet::WalletReader>,
        )]));
        // 6 of the 10-sat run cap are already settled (5 + fee 1).
        let order2 = OrderReference::new("o2");
        let reservation = ctx.budget.lock().unwrap().reserve("o2", 3).unwrap();
        ctx.record_pending_hold(
            "o2",
            PendingHold {
                actor: Actor::Alice,
                wallet_label: "alice".into(),
                payment_hash: "bb".into(),
                reservation,
            },
        );

        expect_hold_refunded(&ctx, &order2).await.unwrap();

        let budget = ctx.budget.lock().unwrap();
        assert_eq!(budget.ledger()[1].status, EntryStatus::Released);
    }

    struct SettledWallet;
    #[async_trait::async_trait]
    impl mortsom_observers::wallet::WalletReader for SettledWallet {
        async fn lookup(&self, _: &str) -> Result<Option<(u64, u64, PaymentState)>, String> {
            Ok(Some((5, 1, PaymentState::Settled)))
        }
    }

    struct FailedWallet;
    #[async_trait::async_trait]
    impl mortsom_observers::wallet::WalletReader for FailedWallet {
        async fn lookup(&self, _: &str) -> Result<Option<(u64, u64, PaymentState)>, String> {
            Ok(Some((5, 0, PaymentState::Failed)))
        }
    }

    /// The sats left the wallet even when the ledger refuses the amount, so
    /// the reservation may not stay `Reserved` and drop out of the spend.
    #[test]
    fn a_refused_settlement_is_still_accounted_for() {
        use crate::budget::{Budget, EntryStatus, SpendCaps};
        use mortsom_observers::wallet::{PaymentObservation, PaymentState};

        let mut budget = Budget::new(SpendCaps::new(100, 100).unwrap());
        let reservation = budget.reserve("o1", 10).unwrap();
        let observation = PaymentObservation {
            wallet: "alice".into(),
            order_id: "o1".into(),
            payment_hash: "aa".into(),
            amount_sats: 50,
            fee_sats: 1,
            state: PaymentState::Settled,
            observed_at: 1,
        };

        let refused = commit_settlement(&mut budget, &reservation, &observation, "aa").unwrap_err();

        assert!(matches!(
            refused,
            crate::budget::BudgetError::Overspend { .. }
        ));
        let entry = &budget.ledger()[0];
        assert_eq!(entry.status, EntryStatus::InFlight, "{entry:?}");
        assert_eq!(entry.payment_hash.as_deref(), Some("aa"));
        assert!(budget.totals().reserved_sats > 0, "still counted as spend");
    }
}
