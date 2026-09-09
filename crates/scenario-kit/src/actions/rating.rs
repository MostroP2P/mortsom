//! Rating after a completed trade: the star given through the UI, and the
//! daemon's own reputation event as the proof it was received.
use mortsom_android_driver::avd::Actor;
use mortsom_app_contract::OrderReference;
use mortsom_observers::relay::ObservedEvent;

use crate::context::{Context, ScenarioError};
use crate::timeline::TimelineKind;

/// Wire kind of the daemon's replaceable user-reputation event
/// (`NOSTR_RATING_EVENT_KIND` in mostro-core).
pub const RATING_EVENT_KIND: u16 = 38384;

/// Rates the counterpart of `order` as `actor` with `score` stars.
pub async fn rate_counterpart(
    ctx: &Context,
    actor: Actor,
    order: &OrderReference,
    score: u8,
) -> Result<(), ScenarioError> {
    let session = ctx.actor(actor)?;
    ctx.adapter
        .rate_counterpart(&session.device(ctx.ui_timeout), order, score)
        .await?;
    let step = ctx
        .evidence
        .checkpoint(actor.as_str(), "rate", session.ui.as_ref())
        .await;
    ctx.timeline.push(
        TimelineKind::Step,
        Some(actor.as_str()),
        format!(
            "rated the counterpart of order {} with {score} stars",
            order.id
        ),
        Some(step),
    );
    Ok(())
}

/// Correlation: the daemon publishes a reputation event after `since`,
/// the relay sequence recorded before the rating was sent. The event names
/// the rated trade key, which the run never learns, so the proof is one
/// published on the run relay after the star was given, not before.
pub async fn wait_rating_published(
    ctx: &Context,
    since: u64,
) -> Result<ObservedEvent, ScenarioError> {
    let daemon = ctx.env.mostro_pubkey.clone();
    let event = ctx
        .relay
        .wait_for(ctx.state_timeout, |e| {
            e.seq > since && e.kind == RATING_EVENT_KIND && e.author == daemon
        })
        .await
        .ok_or_else(|| {
            ScenarioError::relay(format!(
                "the daemon published no rating event within {:?} of the rating",
                ctx.state_timeout
            ))
        })?;
    ctx.timeline.push_at(
        event.received_at_ms,
        TimelineKind::RelayEvent,
        None,
        format!(
            "relay: the daemon published a reputation update (event {})",
            super::short(&event.id)
        ),
        Some(event.id.clone()),
    );
    Ok(event)
}
