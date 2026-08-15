//! Scenario execution: timeout, panic isolation, the single infrastructure
//! retry (spec §10), and the cleanup sweep after each scenario.

use std::panic::AssertUnwindSafe;
use std::time::{Duration, Instant};

use futures::FutureExt;
use mortsom_common::FailureCategory;
use mortsom_scenario_kit::cleanup::CleanupResult;
use mortsom_scenario_kit::context::Context;
use mortsom_scenario_kit::outcome::{Outcome, ScenarioResult};
use mortsom_scenario_kit::registry::ScenarioDef;
use mortsom_scenario_kit::timeline::TimelineKind;
use tracing::{info, warn};

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

/// Cancels every registered order that the scenario did not resolve, on
/// the maker's device, recording each outcome. Idempotent per order: only
/// pending entries are visited, so repeated sweeps do nothing extra.
pub async fn sweep_cleanup(ctx: &Context) {
    for entry in ctx.cleanup.pending() {
        let result = match ctx.actor(entry.actor) {
            Ok(session) => match ctx
                .adapter
                .cancel_if_open(&session.device(ctx.ui_timeout), &entry.order)
                .await
            {
                Ok(outcome) => CleanupResult::Done { outcome },
                Err(e) => CleanupResult::Failed {
                    message: e.to_string(),
                },
            },
            Err(e) => CleanupResult::Failed {
                message: e.to_string(),
            },
        };
        ctx.timeline.push(
            TimelineKind::Cleanup,
            Some(entry.actor.as_str()),
            format!("cleanup order {}: {:?}", entry.order.id, result),
            None,
        );
        ctx.cleanup.record(&entry.order.id, result);
    }
}
