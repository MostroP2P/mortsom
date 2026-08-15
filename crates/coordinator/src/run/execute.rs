//! Scenario execution: timeout, panic isolation, the single infrastructure
//! retry (spec §10), and the cleanup sweep after each scenario.

use std::panic::AssertUnwindSafe;
use std::time::{Duration, Instant};

use futures::FutureExt;
use mortsom_android_driver::avd::Actor;
use mortsom_app_contract::{CancelOutcome, OrderReference, OrderState};
use mortsom_common::FailureCategory;
use mortsom_scenario_kit::cleanup::CleanupResult;
use mortsom_scenario_kit::context::Context;
use mortsom_scenario_kit::outcome::{Outcome, ScenarioResult};
use mortsom_scenario_kit::registry::ScenarioDef;
use mortsom_scenario_kit::timeline::TimelineKind;
use mortsom_scenario_kit::waits::wait_until;
use tracing::{info, warn};

/// Poll interval while waiting for a cancelled order to show as closed.
const CLEANUP_POLL: Duration = Duration::from_millis(500);

/// Runs `def` against `ctx`, retrying once only for a classified
/// infrastructure failure when `infra_retries` allows it.
pub async fn execute_scenario(
    def: &ScenarioDef,
    ctx: &Context,
    infra_retries: u32,
    app_profile: &str,
    app_version: Option<String>,
) -> ScenarioResult {
    let mut attempts = 0;
    loop {
        attempts += 1;
        let started = Instant::now();
        let ledger_before = ledger_len(ctx);
        ctx.timeline.push(
            TimelineKind::Step,
            None,
            format!("scenario {} attempt {attempts} started", def.name),
            None,
        );
        let outcome = attempt(def, ctx).await;
        let duration = started.elapsed();
        ctx.timeline.push(
            TimelineKind::Assertion,
            None,
            format!(
                "scenario {} attempt {attempts}: {}",
                def.name,
                describe(&outcome)
            ),
            None,
        );
        let retryable =
            matches!(&outcome, Outcome::Failed { category, .. } if category.is_retryable());
        // A retry may never spend more: once the failed attempt reserved or
        // moved sats, the failure is final (spec §5, §10).
        let moved_value = ledger_len(ctx) > ledger_before;
        if retryable && attempts <= infra_retries && !moved_value {
            warn!(
                scenario = def.name,
                attempt = attempts,
                "infrastructure failure; retrying once"
            );
            sweep_cleanup(ctx).await;
            continue;
        }
        if retryable && moved_value {
            warn!(
                scenario = def.name,
                "infrastructure failure after a payment; not retrying"
            );
            ctx.timeline.push(
                TimelineKind::Assertion,
                None,
                format!(
                    "scenario {}: retry refused, the attempt already moved sats",
                    def.name
                ),
                None,
            );
        }
        info!(scenario = def.name, attempts, outcome = %describe(&outcome), "scenario finished");
        return ScenarioResult {
            name: def.name.into(),
            app_profile: app_profile.into(),
            app_version,
            outcome,
            attempts,
            duration,
        };
    }
}

async fn attempt(def: &ScenarioDef, ctx: &Context) -> Outcome {
    let timeout = Duration::from_secs(def.timeout_secs);
    let body = AssertUnwindSafe((def.run)(ctx)).catch_unwind();
    match tokio::time::timeout(timeout, body).await {
        Ok(Ok(Ok(()))) => Outcome::Passed,
        Ok(Ok(Err(e))) => Outcome::Failed {
            category: e.category,
            message: e.message,
        },
        Ok(Err(_panic)) => Outcome::Failed {
            category: FailureCategory::TestInfrastructure,
            message: "scenario panicked; see run.log".into(),
        },
        Err(_) => Outcome::Failed {
            category: FailureCategory::Product,
            message: format!("scenario exceeded its {timeout:?} deadline"),
        },
    }
}

fn ledger_len(ctx: &Context) -> usize {
    ctx.budget
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .ledger()
        .len()
}

fn describe(outcome: &Outcome) -> String {
    match outcome {
        Outcome::Passed => "passed".into(),
        Outcome::Failed { category, message } => format!("failed ({category}): {message}"),
        Outcome::Skipped { reason } => format!("skipped: {reason}"),
    }
}

/// Cancels every registered order that the scenario did not resolve and
/// verifies the result on screen: the maker cancels first; when the order
/// is funded (both sides must agree) the counterpart cancels too. An order
/// still open afterwards is recorded as failed, never as done. Idempotent
/// per order: only pending entries are visited.
pub async fn sweep_cleanup(ctx: &Context) {
    for entry in ctx.cleanup.pending() {
        let result = cleanup_order(ctx, entry.actor, &entry.order).await;
        ctx.timeline.push(
            TimelineKind::Cleanup,
            Some(entry.actor.as_str()),
            format!("cleanup order {}: {:?}", entry.order.id, result),
            None,
        );
        ctx.cleanup.record(&entry.order.id, result);
    }
}

async fn cleanup_order(ctx: &Context, maker: Actor, order: &OrderReference) -> CleanupResult {
    let outcome = match cancel_as(ctx, maker, order).await {
        Ok(outcome) => outcome,
        Err(message) => return CleanupResult::Failed { message },
    };
    if outcome != CancelOutcome::Cancelled {
        return CleanupResult::Done { outcome };
    }
    // The maker's request alone closes an unfunded order; a funded one
    // needs the counterpart's agreement.
    match observed_state(ctx, maker, order).await {
        Some(state) if state.is_terminal() => return CleanupResult::Done { outcome },
        Some(OrderState::Active | OrderState::FiatSent) => {}
        Some(state) => {
            return CleanupResult::Failed {
                message: format!("still {state} after the maker cancelled"),
            };
        }
        None => {
            return CleanupResult::Failed {
                message: "state unreadable after the maker cancelled".into(),
            };
        }
    }
    let Some(taker) = Actor::ALL.into_iter().find(|a| *a != maker) else {
        return CleanupResult::Failed {
            message: "no counterpart to accept the cancel".into(),
        };
    };
    match cancel_as(ctx, taker, order).await {
        Ok(_) => {}
        Err(message) => return CleanupResult::Failed { message },
    }
    match observed_state(ctx, maker, order).await {
        Some(state) if state.is_terminal() => CleanupResult::Done { outcome },
        Some(state) => CleanupResult::Failed {
            message: format!("still {state} after both sides cancelled"),
        },
        None => CleanupResult::Failed {
            message: "state unreadable after both sides cancelled".into(),
        },
    }
}

async fn cancel_as(
    ctx: &Context,
    actor: Actor,
    order: &OrderReference,
) -> Result<CancelOutcome, String> {
    let session = ctx.actor(actor).map_err(|e| e.to_string())?;
    ctx.adapter
        .cancel_if_open(&session.device(ctx.ui_timeout), order)
        .await
        .map_err(|e| e.to_string())
}

/// The state `actor`'s app shows once it stops changing, bounded by the
/// state timeout; `None` when it cannot be read.
async fn observed_state(ctx: &Context, actor: Actor, order: &OrderReference) -> Option<OrderState> {
    let session = ctx.actor(actor).ok()?;
    let device = session.device(ctx.ui_timeout);
    wait_until(ctx.state_timeout, CLEANUP_POLL, || async {
        match ctx.adapter.observe_order(&device, order).await {
            Ok(state) if state.state.is_terminal() => Some(state.state),
            _ => None,
        }
    })
    .await
    .or(ctx
        .adapter
        .observe_order(&device, order)
        .await
        .ok()
        .map(|s| s.state))
}
