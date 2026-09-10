//! Disputes: opened through the UI, ruled on by the run's solver. The
//! proof of each step is the daemon's public dispute event, not a screen:
//! the event names the dispute id (which the run learns only from it) and
//! carries the dispute status the daemon moved it to.
use mortsom_app_contract::OrderReference;
use mortsom_common::Actor;
use mortsom_observers::relay::ObservedEvent;

use crate::context::{Context, ScenarioError};
use crate::timeline::TimelineKind;

use super::short;

/// Wire kind of the daemon's replaceable dispute event
/// (`NOSTR_DISPUTE_EVENT_KIND` in mostro-core).
pub const DISPUTE_EVENT_KIND: u16 = 38386;

/// The daemon's dispute statuses, as carried in the event's `s` tag.
pub mod dispute_status {
    /// Opened by a party; no solver yet.
    pub const INITIATED: &str = "initiated";
    /// A solver took it.
    pub const IN_PROGRESS: &str = "in-progress";
    /// The solver cancelled the order; the escrow went back to the seller.
    pub const SELLER_REFUNDED: &str = "seller-refunded";
    /// The solver settled the order; the escrow went to the buyer.
    pub const SETTLED: &str = "settled";
    /// The seller released while the dispute was open.
    pub const RELEASED: &str = "released";
}

/// A dispute the daemon has published.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Dispute {
    /// The dispute id, from the event's `d` tag.
    pub id: String,
    /// Who opened it, from the event's `initiator` tag (`buyer`/`seller`).
    pub initiator: String,
}

/// Opens a dispute on `order` as `actor` through the UI.
pub async fn open_dispute(
    ctx: &Context,
    actor: Actor,
    order: &OrderReference,
) -> Result<(), ScenarioError> {
    let session = ctx.actor(actor)?;
    ctx.adapter
        .open_dispute(&session.device(ctx.ui_timeout), order)
        .await?;
    let step = ctx
        .evidence
        .checkpoint(actor.as_str(), "dispute", session.ui.as_ref())
        .await;
    ctx.timeline.push(
        TimelineKind::Step,
        Some(actor.as_str()),
        format!("opened a dispute on order {}", order.id),
        Some(step),
    );
    Ok(())
}

/// Correlation: the daemon publishes a dispute as `initiated` after
/// `since`, the relay sequence recorded before the dispute was opened.
/// The event never names the order, so the proof is one published on the
/// run relay after the request, not before; the id it carries is what the
/// solver takes.
pub async fn wait_dispute_opened(ctx: &Context, since: u64) -> Result<Dispute, ScenarioError> {
    let event = wait_dispute_event(ctx, since, None, dispute_status::INITIATED).await?;
    let id = event
        .tag("d")
        .ok_or_else(|| ScenarioError::relay("the dispute event carries no dispute id"))?
        .to_owned();
    let initiator = event.tag("initiator").unwrap_or("unknown").to_owned();
    ctx.timeline.push_at(
        event.received_at_ms,
        TimelineKind::RelayEvent,
        None,
        format!(
            "relay: dispute {} opened by the {initiator} (event {})",
            short(&id),
            short(&event.id)
        ),
        Some(event.id.clone()),
    );
    Ok(Dispute { id, initiator })
}

/// Correlation: the daemon publishes dispute `dispute_id` as `status`
/// after `since`.
pub async fn wait_dispute_status(
    ctx: &Context,
    dispute_id: &str,
    status: &str,
    since: u64,
) -> Result<ObservedEvent, ScenarioError> {
    let event = wait_dispute_event(ctx, since, Some(dispute_id), status).await?;
    ctx.timeline.push_at(
        event.received_at_ms,
        TimelineKind::RelayEvent,
        None,
        format!(
            "relay: dispute {} is {status} (event {})",
            short(dispute_id),
            short(&event.id)
        ),
        Some(event.id.clone()),
    );
    Ok(event)
}

async fn wait_dispute_event(
    ctx: &Context,
    since: u64,
    dispute_id: Option<&str>,
    status: &str,
) -> Result<ObservedEvent, ScenarioError> {
    let daemon = ctx.env.mostro_pubkey.clone();
    ctx.relay
        .wait_for(ctx.state_timeout, |e| {
            e.seq > since
                && e.kind == DISPUTE_EVENT_KIND
                && e.author == daemon
                && e.tag("s") == Some(status)
                && dispute_id.is_none_or(|id| e.tag("d") == Some(id))
        })
        .await
        .ok_or_else(|| {
            ScenarioError::relay(format!(
                "the daemon published no dispute {}as {status} within {:?}",
                dispute_id
                    .map(|id| format!("{} ", short(id)))
                    .unwrap_or_default(),
                ctx.state_timeout
            ))
        })
}

/// The run's solver takes `dispute`, which must be about `order`: the
/// daemon's answer names the order the dispute is attached to, and a
/// solver taking somebody else's dispute is not what the scenario meant.
pub async fn solver_takes_dispute(
    ctx: &Context,
    dispute: &Dispute,
    order: &OrderReference,
) -> Result<(), ScenarioError> {
    let reported = ctx.solver()?.take_dispute(&dispute.id).await.map_err(|e| {
        ScenarioError::product(format!("the solver could not take the dispute: {e}"))
    })?;
    if reported != order.id {
        return Err(ScenarioError::product(format!(
            "dispute {} is about order {reported}, not {}",
            short(&dispute.id),
            order.id
        )));
    }
    ctx.timeline.push(
        TimelineKind::Step,
        Some("solver"),
        format!(
            "the solver took dispute {} on order {}",
            short(&dispute.id),
            order.id
        ),
        None,
    );
    Ok(())
}

/// The run's solver settles `order` in the buyer's favour.
pub async fn solver_settles(ctx: &Context, order: &OrderReference) -> Result<(), ScenarioError> {
    ctx.solver()?
        .settle(&order.id)
        .await
        .map_err(|e| ScenarioError::product(format!("the solver could not settle: {e}")))?;
    ctx.timeline.push(
        TimelineKind::Step,
        Some("solver"),
        format!("the solver settled order {} for the buyer", order.id),
        None,
    );
    Ok(())
}

/// The run's solver cancels `order` in the seller's favour.
pub async fn solver_cancels(ctx: &Context, order: &OrderReference) -> Result<(), ScenarioError> {
    ctx.solver()?
        .cancel(&order.id)
        .await
        .map_err(|e| ScenarioError::product(format!("the solver could not cancel: {e}")))?;
    ctx.timeline.push(
        TimelineKind::Step,
        Some("solver"),
        format!("the solver cancelled order {} for the seller", order.id),
        None,
    );
    Ok(())
}
