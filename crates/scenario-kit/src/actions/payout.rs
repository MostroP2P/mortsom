//! Payouts the daemon cannot pay, and payouts one payee pins: withdrawing
//! or refusing a buyer invoice, proving a superseded invoice never
//! settled, and driving hold invoices on the buyer's node so the daemon's
//! per-destination cap can be observed from the outside.
use std::time::Duration;

use mortsom_app_contract::{InvoiceSubmission, OrderReference};
use mortsom_common::Actor;
use mortsom_observers::wallet::PaymentState;

use super::{STATE_POLL, buyer_invoice, short};
use crate::context::{Context, HoldInvoiceState, NodeWallet, ScenarioError};
use crate::journal::RecoveryEvent;
use crate::timeline::TimelineKind;
use crate::waits::{stays_absent, wait_until};

/// Who refused a buyer invoice, in their own terms.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refusal {
    /// The app judged it and never sent it: its `invoice.check` word.
    App {
        /// The rule the app named, e.g. `expires-too-soon`.
        check: String,
    },
    /// The daemon refused it: the reason the app then showed, as prose.
    Daemon {
        /// The displayed reason; evidence, never compared.
        reason: String,
    },
}

/// A buyer invoice that was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RejectedInvoice {
    /// Payment hash of the refused invoice.
    pub hash: String,
    /// Who refused it.
    pub refusal: Refusal,
}

/// A buyer invoice the daemon took.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AcceptedInvoice {
    /// Payment hash of the invoice, the payout's correlation key.
    pub hash: String,
    /// The app's own verdict when it was sent, or `None` if it had none.
    pub check: Option<String>,
}

fn node_of(ctx: &Context, actor: Actor) -> Result<&std::sync::Arc<dyn NodeWallet>, ScenarioError> {
    ctx.node_wallet(actor).ok_or_else(|| {
        ScenarioError::environment(format!(
            "this step needs a node Mortsom holds for {actor}, and the run has none"
        ))
    })
}

/// Hands in a buyer invoice of `kind` as `actor` that must be refused.
/// Either refusal is the answer: the app's own, before anything is sent,
/// or the daemon's, with the form kept open and the reason on screen. The
/// app judges first once the node's capabilities reach it; before that the
/// daemon is the only judge. The refused invoice is withdrawn on the node
/// and superseded in the journal, so a proper one can follow.
pub async fn submit_rejected_buyer_invoice(
    ctx: &Context,
    actor: Actor,
    order: &OrderReference,
    kind: buyer_invoice::BuyerInvoice,
) -> Result<RejectedInvoice, ScenarioError> {
    let session = ctx.actor(actor)?;
    let (invoice, hash) = issued(ctx, actor, order, kind).await?;
    let verdict = ctx
        .adapter
        .submit_buyer_invoice(&session.device(ctx.ui_timeout), order, &invoice)
        .await?;
    let (refusal, said) = match verdict {
        InvoiceSubmission::RefusedByApp { check } => (
            Refusal::App {
                check: check.clone(),
            },
            format!("by the app before sending it (`{check}`)"),
        ),
        InvoiceSubmission::Rejected { reason } => (
            Refusal::Daemon {
                reason: reason.clone(),
            },
            format!("by the daemon: {reason}"),
        ),
        InvoiceSubmission::Accepted { .. } => {
            return Err(ScenarioError::product(format!(
                "the daemon accepted a buyer invoice for order {} it should have refused ({kind:?})",
                order.id
            )));
        }
    };
    let step = ctx
        .evidence
        .checkpoint(actor.as_str(), "buyer-invoice-refused", session.ui.as_ref())
        .await;
    ctx.timeline.push(
        TimelineKind::Step,
        Some(actor.as_str()),
        format!(
            "buyer invoice for order {} (hash {}) refused {said}",
            order.id,
            short(&hash)
        ),
        Some(step),
    );
    supersede_buyer_invoice(ctx, actor, order, &hash).await?;
    Ok(RejectedInvoice { hash, refusal })
}

/// Hands in a proper buyer invoice of `kind` as `actor`, which the daemon
/// must take, and reports the app's own verdict on it as it was sent.
pub async fn submit_accepted_buyer_invoice(
    ctx: &Context,
    actor: Actor,
    order: &OrderReference,
    kind: buyer_invoice::BuyerInvoice,
) -> Result<AcceptedInvoice, ScenarioError> {
    let session = ctx.actor(actor)?;
    let (invoice, hash) = issued(ctx, actor, order, kind).await?;
    let verdict = ctx
        .adapter
        .submit_buyer_invoice(&session.device(ctx.ui_timeout), order, &invoice)
        .await?;
    let check = match verdict {
        InvoiceSubmission::Accepted { check } => check,
        InvoiceSubmission::RefusedByApp { check } => {
            return Err(ScenarioError::product(format!(
                "the app refused a proper buyer invoice for order {} (`{check}`)",
                order.id
            )));
        }
        InvoiceSubmission::Rejected { reason } => {
            return Err(ScenarioError::product(format!(
                "the daemon refused a proper buyer invoice for order {}: {reason}",
                order.id
            )));
        }
    };
    let step = ctx
        .evidence
        .checkpoint(actor.as_str(), "buyer-invoice", session.ui.as_ref())
        .await;
    ctx.timeline.push(
        TimelineKind::Step,
        Some(actor.as_str()),
        format!(
            "provided buyer invoice for order {} (hash {}), app verdict {}",
            order.id,
            short(&hash),
            check
                .as_deref()
                .map_or_else(|| "none".to_owned(), |word| format!("`{word}`"))
        ),
        Some(step),
    );
    Ok(AcceptedInvoice { hash, check })
}

/// Issues a buyer invoice of `kind` on `actor`'s node, journaled as the
/// payment expected for `order`, with its payment hash.
async fn issued(
    ctx: &Context,
    actor: Actor,
    order: &OrderReference,
    kind: buyer_invoice::BuyerInvoice,
) -> Result<(mortsom_common::Secret, String), ScenarioError> {
    let prepared = buyer_invoice::prepare(ctx, actor, order, kind).await?;
    match (prepared.invoice.as_ref(), prepared.hash()) {
        (Some(invoice), Some(hash)) => Ok((invoice.clone(), hash.to_owned())),
        _ => Err(ScenarioError::environment(
            "a judged invoice must be one Mortsom issued on a node it holds",
        )),
    }
}

/// Withdraws the buyer invoice `hash` on `actor`'s node: superseded in the
/// journal first, so it is never expected to settle, then cancelled on the
/// node. A payment the daemon makes to it afterwards fails.
pub async fn supersede_buyer_invoice(
    ctx: &Context,
    actor: Actor,
    order: &OrderReference,
    hash: &str,
) -> Result<(), ScenarioError> {
    let node = node_of(ctx, actor)?;
    ctx.journal
        .record(RecoveryEvent::PaymentSuperseded {
            order_id: order.id.clone(),
            wallet: actor.as_str().into(),
            hash: hash.to_owned(),
        })
        .map_err(ScenarioError::environment)?;
    node.cancel_invoice(hash)
        .await
        .map_err(ScenarioError::wallet)?;
    ctx.timeline.push(
        TimelineKind::Wallet,
        Some(actor.as_str()),
        format!(
            "wallet: buyer invoice {} for order {} withdrawn",
            short(hash),
            order.id
        ),
        Some(hash.to_owned()),
    );
    Ok(())
}

/// A superseded payout must never have been paid: the node shows the
/// invoice failed and holds no settled record under its hash.
pub async fn expect_payout_never_settled(
    ctx: &Context,
    actor: Actor,
    order: &OrderReference,
    hash: &str,
) -> Result<(), ScenarioError> {
    let session = ctx.actor(actor)?;
    let obs = ctx
        .wallets
        .wait_terminal(&session.wallet_label, &order.id, hash, ctx.settle_timeout)
        .await
        .map_err(|e| ScenarioError::wallet(e.to_string()))?;
    let records = node_of(ctx, actor)?
        .settled_record_count(hash, true)
        .await
        .map_err(ScenarioError::wallet)?;
    if obs.state != PaymentState::Failed || records != 0 {
        return Err(ScenarioError::product(format!(
            "superseded payout {} for order {} ended {:?} with {records} settled records; it must never be paid",
            short(hash),
            order.id,
            obs.state
        )));
    }
    ctx.timeline.push_at(
        obs.observed_at * 1000,
        TimelineKind::Wallet,
        Some(actor.as_str()),
        format!(
            "wallet: superseded payout {} for order {} never settled",
            short(hash),
            order.id
        ),
        Some(hash.to_owned()),
    );
    Ok(())
}

async fn hold_state(
    ctx: &Context,
    actor: Actor,
    hash: &str,
) -> Result<HoldInvoiceState, ScenarioError> {
    node_of(ctx, actor)?
        .hold_invoice_state(hash)
        .await
        .map_err(ScenarioError::wallet)
}

/// Waits until the hold invoice `hash` on `actor`'s node reports
/// `expected`, bounded by `timeout`. A hold invoice that turns `accepted`
/// is a payout the daemon dispatched and the node is now holding.
pub async fn wait_hold_invoice(
    ctx: &Context,
    actor: Actor,
    order: &OrderReference,
    hash: &str,
    expected: HoldInvoiceState,
    timeout: Duration,
) -> Result<(), ScenarioError> {
    let found = wait_until(timeout, STATE_POLL, || async {
        match hold_state(ctx, actor, hash).await {
            Ok(state) if state == expected => Some(Ok(())),
            Ok(HoldInvoiceState::Cancelled) => Some(Err(ScenarioError::wallet(format!(
                "hold invoice {} for order {} was cancelled while waiting for {}",
                short(hash),
                order.id,
                expected.as_str()
            )))),
            Ok(_) => None,
            Err(e) => Some(Err(e)),
        }
    })
    .await;
    match found {
        Some(Ok(())) => {
            ctx.timeline.push(
                TimelineKind::Wallet,
                Some(actor.as_str()),
                format!(
                    "wallet: hold invoice {} for order {} is {}",
                    short(hash),
                    order.id,
                    expected.as_str()
                ),
                Some(hash.to_owned()),
            );
            Ok(())
        }
        Some(Err(e)) => Err(e),
        None => Err(ScenarioError::product(format!(
            "hold invoice {} for order {} did not become {} within {timeout:?}",
            short(hash),
            order.id,
            expected.as_str()
        ))),
    }
}

/// Negative observation: for the whole `window` the hold invoice `hash`
/// stays `expected` -- typically `open`, proving the daemon dispatched no
/// payment to it. Any other state within the window is a product failure.
pub async fn expect_hold_invoice_stays(
    ctx: &Context,
    actor: Actor,
    order: &OrderReference,
    hash: &str,
    expected: HoldInvoiceState,
    window: Duration,
) -> Result<(), ScenarioError> {
    let departed = stays_absent(window, STATE_POLL, || async {
        match hold_state(ctx, actor, hash).await {
            Ok(state) if state == expected => None,
            Ok(state) => Some(Ok(state)),
            Err(e) => Some(Err(e)),
        }
    })
    .await;
    match departed {
        None => {
            ctx.timeline.push(
                TimelineKind::Assertion,
                Some(actor.as_str()),
                format!(
                    "wallet: hold invoice {} for order {} stayed {} for {window:?}",
                    short(hash),
                    order.id,
                    expected.as_str()
                ),
                Some(hash.to_owned()),
            );
            Ok(())
        }
        Some(Ok(state)) => Err(ScenarioError::product(format!(
            "hold invoice {} for order {} became {} within {window:?}; it had to stay {}",
            short(hash),
            order.id,
            state.as_str(),
            expected.as_str()
        ))),
        Some(Err(e)) => Err(e),
    }
}

/// Leaves no payout pinned behind a failed scenario: every hold invoice in
/// `held` that is holding a payment is settled (the escrow it pays for was
/// already settled, so paying the buyer is the honest end), and every one
/// still untouched is withdrawn so nothing can be paid to it later. Errors
/// are collected, not raised: the caller is already failing.
pub async fn release_held_payouts(
    ctx: &Context,
    actor: Actor,
    held: &[(OrderReference, String)],
) -> Vec<String> {
    let mut problems = Vec::new();
    for (order, hash) in held {
        let outcome = match hold_state(ctx, actor, hash).await {
            Ok(HoldInvoiceState::Accepted) => {
                settle_buyer_hold_invoice(ctx, actor, order, hash).await
            }
            Ok(HoldInvoiceState::Open) => supersede_buyer_invoice(ctx, actor, order, hash).await,
            Ok(_) => Ok(()),
            Err(e) => Err(e),
        };
        if let Err(e) = outcome {
            problems.push(format!("order {}: {e}", order.id));
        }
    }
    problems
}

/// Settles the hold invoice `hash` on `actor`'s node, releasing the payout
/// the daemon has been holding there.
pub async fn settle_buyer_hold_invoice(
    ctx: &Context,
    actor: Actor,
    order: &OrderReference,
    hash: &str,
) -> Result<(), ScenarioError> {
    node_of(ctx, actor)?
        .settle_hold_invoice(hash)
        .await
        .map_err(ScenarioError::wallet)?;
    ctx.timeline.push(
        TimelineKind::Step,
        Some(actor.as_str()),
        format!(
            "settled hold invoice {} for order {}",
            short(hash),
            order.id
        ),
        Some(hash.to_owned()),
    );
    Ok(())
}
