//! Domain actions (spec §7): each finishes only after its expected visible
//! state appears and, when appropriate, its external consequence is
//! observed. Every action records evidence and a timeline entry.

#[path = "actions/hold_budget.rs"]
mod hold_budget;
use std::fmt;
use std::sync::Mutex;
use std::time::Duration;

use mortsom_android_driver::avd::Actor;
use mortsom_app_contract::{OrderDraft, OrderReference, OrderState, PaymentPath};
use mortsom_observers::wallet::{PaymentObservation, PaymentState, payment_hash_of};
use tracing::info;

use crate::context::{Context, PendingHold, ScenarioError};
use crate::journal::RecoveryEvent;
use crate::timeline::TimelineKind;
use crate::waits::{stays_absent, wait_until};

/// Poll interval for state observations.
pub const STATE_POLL: Duration = Duration::from_secs(3);
/// Wire kind of order events.
/// Kind of the daemon's public, replaceable order event.
pub const ORDER_EVENT_KIND: u16 = 38383;

/// Creates an order as `actor` and registers its cleanup immediately.
pub async fn create_order(
    ctx: &Context,
    actor: Actor,
    draft: &OrderDraft,
) -> Result<OrderReference, ScenarioError> {
    let session = ctx.actor(actor)?;
    if ctx.journal.enabled() {
        let known_orders = ctx
            .adapter
            .owned_orders(&session.device(ctx.ui_timeout))
            .await?;
        ctx.journal
            .record(RecoveryEvent::SubmissionStarted {
                actor,
                known_orders,
            })
            .map_err(ScenarioError::environment)?;
    }
    let visible = ctx
        .adapter
        .create_order(&session.device(ctx.ui_timeout), draft)
        .await?;
    let id = visible
        .id
        .ok_or_else(|| ScenarioError::product("the app shows no id for the created order"))?;
    let order = OrderReference::with_kind(id.clone(), draft.kind);
    ctx.cleanup.register(actor, &order);
    ctx.journal
        .record(RecoveryEvent::OrderFound {
            actor,
            order: order.clone(),
        })
        .map_err(ScenarioError::environment)?;
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
    let last: Mutex<LastSeen> = Mutex::new(LastSeen::default());
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
                last.lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .read(state.raw_label);
                None
            }
            // An observation that failed is not the app showing the wrong
            // state: it is kept beside the last status the app did render,
            // so the deadline can say which of the two it ended on.
            Err(e) => {
                last.lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .failed(e.to_string());
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
                &last.lock().unwrap_or_else(|p| p.into_inner()),
            )))
        }
    }
}

/// Waits until `actor`'s trade detail renders one of the raw `labels` for
/// `order`, bounded by the state timeout. Finer than [`wait_app_state`]:
/// several daemon statuses map to one [`OrderState`], and a journey sometimes
/// has to wait for a particular one -- the buyer may only hand in a payout
/// invoice once the daemon has asked for it, which the detail shows as
/// `waiting-invoice` while the coarser state is still `taken`. The detail is
/// read on purpose: an open form reports a fixed waiting label for as long as
/// it is up, so it cannot prove that the daemon's request has arrived.
pub async fn wait_app_label(
    ctx: &Context,
    actor: Actor,
    order: &OrderReference,
    labels: &[&str],
) -> Result<String, ScenarioError> {
    let session = ctx.actor(actor)?;
    let device = session.device(ctx.ui_timeout);
    let normalize = |raw: &str| raw.trim().to_ascii_lowercase().replace(['_', ' '], "-");
    let found = wait_until(ctx.state_timeout, STATE_POLL, || async {
        match ctx.adapter.observe_order_detail(&device, order).await {
            Ok(state) if labels.contains(&normalize(&state.raw_label).as_str()) => {
                Some(Ok(state.raw_label))
            }
            Ok(state) if state.state.is_terminal() => Some(Err(ScenarioError::product(format!(
                "order {} reached terminal `{}` while waiting for {}",
                order.id,
                state.raw_label,
                labels.join("|")
            )))),
            _ => None,
        }
    })
    .await;
    match found {
        Some(result) => result,
        None => Err(ScenarioError::product(format!(
            "order {} did not show {} on {} within {:?}",
            order.id,
            labels.join("|"),
            actor.as_str(),
            ctx.state_timeout
        ))),
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

/// What the observations of an order said before the deadline.
///
/// A wait that ends without its state has to say which of the two happened:
/// the app showed something else, or the app was never read at all. They
/// point at opposite halves of the system, and a message that omits it
/// blames the product for what may be a broken navigation.
///
/// The two are kept apart rather than overwriting each other, because an
/// intermittent failure produces both: a read that fails does not unsee the
/// status an earlier one read, and a wait that ends that way is neither
/// story on its own. A successful read does clear the failure -- a blip the
/// next poll answered is not what the deadline is about.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LastSeen {
    /// The last raw status label the app rendered, if it was ever read.
    status: Option<String>,
    /// Why the most recent observation failed, if it was the last word.
    failure: Option<String>,
}

impl LastSeen {
    /// Records a status the app rendered; the failures before it are stale.
    fn read(&mut self, raw: String) {
        self.status = Some(raw);
        self.failure = None;
    }

    /// Records an observation that failed, keeping any status already read.
    fn failed(&mut self, why: String) {
        self.failure = Some(why);
    }
}

impl fmt::Display for LastSeen {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match (&self.status, &self.failure) {
            (Some(raw), None) => write!(f, "last showed `{raw}`"),
            (Some(raw), Some(why)) => {
                write!(f, "last showed `{raw}`, and then could not be read: {why}")
            }
            (None, Some(why)) => write!(f, "was never read: {why}"),
            (None, None) => write!(f, "was never read"),
        }
    }
}

/// The message a state wait leaves when its deadline passes.
fn state_timeout_message(
    order: &OrderReference,
    expected: &str,
    actor: Actor,
    timeout: Duration,
    last: &LastSeen,
) -> String {
    format!(
        "order {} did not reach {expected} on {actor} within {timeout:?}; the app {last}",
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

#[path = "actions/hold.rs"]
mod hold;
pub use hold::{expect_hold_refunded, pay_hold_invoice, settle_hold_payment};

mod buyer_invoice;

/// Generates the buyer invoice as `actor` (no value moves).
pub async fn provide_buyer_invoice(
    ctx: &Context,
    actor: Actor,
    order: &OrderReference,
) -> Result<Option<String>, ScenarioError> {
    let session = ctx.actor(actor)?;
    if ctx.journal.enabled() && ctx.node_wallet(actor).is_none() {
        return Err(ScenarioError::environment(
            "durable desktop recovery requires a caller-controlled invoice wallet",
        ));
    }
    let prepared = buyer_invoice::prepare(ctx, actor, order).await?;
    let invoice = ctx
        .adapter
        .provide_buyer_invoice(
            &session.device(ctx.ui_timeout),
            order,
            prepared.invoice.as_ref(),
        )
        .await?;
    if let Some(invoice) = &invoice {
        ctx.protect(invoice);
    }
    let hash = buyer_invoice::verify_returned(&prepared, invoice.as_ref())?;
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
    if ctx.journal.enabled() {
        let expected = ctx
            .journal
            .events()
            .iter()
            .find_map(|event| match event {
                RecoveryEvent::PaymentExpected {
                    order_id,
                    wallet,
                    hash: expected_hash,
                    incoming: true,
                    amount_sats,
                } if order_id == &order.id && wallet == actor.as_str() && expected_hash == hash => {
                    Some(*amount_sats)
                }
                _ => None,
            })
            .ok_or_else(|| {
                ScenarioError::wallet("payout has no durable expected amount and hash")
            })?;
        if obs.amount_sats != expected || obs.fee_sats != 0 {
            return Err(ScenarioError::wallet(
                "buyer payout amount or receiving fee differs from the prepared invoice",
            ));
        }
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
#[path = "actions/tests.rs"]
mod tests;
