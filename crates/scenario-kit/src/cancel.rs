//! Cancellation and permission actions (spec §9.2, Phase 5): cancelling an
//! order or a pending payment through the UI, and asserting which trade
//! actions the app offers (unauthorized-action UX). Nothing here pays.

use mortsom_android_driver::avd::Actor;
use mortsom_app_contract::{CancelOutcome, OrderReference};

use crate::context::{Context, ScenarioError};
use crate::timeline::TimelineKind;

/// Cancels `order` as `actor` through the trade screen. Returns what the app
/// reported; the caller decides whether `AlreadyClosed`/`NotFound` is a bug.
pub async fn cancel_order(
    ctx: &Context,
    actor: Actor,
    order: &OrderReference,
) -> Result<CancelOutcome, ScenarioError> {
    let session = ctx.actor(actor)?;
    let outcome = ctx
        .adapter
        .cancel_if_open(&session.device(ctx.ui_timeout), order)
        .await?;
    let step = ctx
        .evidence
        .checkpoint(actor.as_str(), "cancel", session.ui.as_ref())
        .await;
    ctx.timeline.push(
        TimelineKind::Step,
        Some(actor.as_str()),
        format!("cancel order {}: {outcome:?}", order.id),
        Some(step),
    );
    Ok(outcome)
}

/// Cancels `order` from the pay-invoice screen instead of paying (the NWC
/// cancellation path).
pub async fn cancel_payment(
    ctx: &Context,
    actor: Actor,
    order: &OrderReference,
) -> Result<(), ScenarioError> {
    let session = ctx.actor(actor)?;
    ctx.adapter
        .cancel_payment(&session.device(ctx.ui_timeout), order)
        .await?;
    let step = ctx
        .evidence
        .checkpoint(actor.as_str(), "cancel-payment", session.ui.as_ref())
        .await;
    ctx.timeline.push(
        TimelineKind::Step,
        Some(actor.as_str()),
        format!("cancelled payment of order {}", order.id),
        Some(step),
    );
    Ok(())
}

/// Asserts the trade actions `actor` can see for `order`: every name in
/// `offered` must be present and none in `forbidden` may be. Fails as a
/// product bug otherwise; the visible set is recorded either way.
pub async fn expect_actions(
    ctx: &Context,
    actor: Actor,
    order: &OrderReference,
    offered: &[&str],
    forbidden: &[&str],
) -> Result<Vec<String>, ScenarioError> {
    let session = ctx.actor(actor)?;
    let visible = ctx
        .adapter
        .visible_actions(&session.device(ctx.ui_timeout), order)
        .await?;
    let step = ctx
        .evidence
        .checkpoint(actor.as_str(), "visible-actions", session.ui.as_ref())
        .await;
    ctx.timeline.push(
        TimelineKind::UiState,
        Some(actor.as_str()),
        format!("order {} offers [{}]", order.id, visible.join(", ")),
        Some(step),
    );
    let missing: Vec<&str> = offered
        .iter()
        .copied()
        .filter(|a| !visible.iter().any(|v| v == a))
        .collect();
    let illegal: Vec<&str> = forbidden
        .iter()
        .copied()
        .filter(|a| visible.iter().any(|v| v == a))
        .collect();
    if !missing.is_empty() || !illegal.is_empty() {
        return Err(ScenarioError::product(format!(
            "order {} on {actor}: missing [{}], illegal [{}], visible [{}]",
            order.id,
            missing.join(", "),
            illegal.join(", "),
            visible.join(", ")
        )));
    }
    Ok(visible)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use std::sync::{Arc, Mutex};

    use mortsom_app_contract::OrderState;

    use super::*;
    use crate::test_support::{StateAdapter, context};

    #[tokio::test]
    async fn cancel_actions_record_steps() {
        let ctx = context(Arc::new(StateAdapter(Mutex::new(OrderState::Pending))));
        let order = OrderReference::new("o1");
        let outcome = cancel_order(&ctx, Actor::Alice, &order).await.unwrap();
        assert_eq!(outcome, CancelOutcome::NotFound);
        cancel_payment(&ctx, Actor::Alice, &order).await.unwrap();
        let entries = ctx.timeline.entries();
        assert!(
            entries
                .iter()
                .any(|e| e.summary.contains("cancel order o1"))
        );
        assert!(
            entries
                .iter()
                .any(|e| e.summary.contains("cancelled payment"))
        );
    }

    #[tokio::test]
    async fn expect_actions_reports_missing_and_illegal() {
        let ctx = context(Arc::new(StateAdapter(Mutex::new(OrderState::Taken))));
        let order = OrderReference::new("o1");
        // The fake offers nothing: forbidding is fine, requiring is not.
        expect_actions(&ctx, Actor::Bob, &order, &[], &["release"])
            .await
            .unwrap();
        let err = expect_actions(&ctx, Actor::Bob, &order, &["fiat-sent"], &[])
            .await
            .unwrap_err();
        assert!(err.message.contains("missing [fiat-sent]"));
    }
}
