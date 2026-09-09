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

/// How long the screen-of-the-failure capture may take, per actor.
///
/// The scenario has already failed and cleanup is queued behind this. When
/// the driver is the component that broke, a WebDriver request sits on its
/// own 60-second HTTP timeout, so an unbounded serial capture could delay
/// returning real sats by minutes and would photograph the second phone
/// long after the moment it is supposed to show.
const FAILURE_CAPTURE_DEADLINE: Duration = Duration::from_secs(15);

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
    let outcome = match tokio::time::timeout(timeout, body).await {
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
    };
    if matches!(outcome, Outcome::Failed { .. }) {
        capture_failure(def, ctx).await;
    }
    outcome
}

/// Records the screen each actor was on when the scenario failed.
///
/// Every checkpoint in the scenario kit sits on the success path: an action
/// that fails returns before reaching it. So a failed run left only its
/// bootstrap screenshots, and the runbook's first instruction -- open the
/// actor's last screen -- had nothing to open, on the one run where it
/// matters. This is the single place that sees every failure, whether it
/// was an assertion, the deadline or a panic.
///
/// Best effort by design: the recorder logs what it cannot capture rather
/// than failing, because a dead session is often exactly what went wrong
/// and must not replace the diagnosis with an error about the diagnosis.
///
/// Both actors are captured concurrently and each under
/// [`FAILURE_CAPTURE_DEADLINE`], so the diagnosis can never postpone the
/// cleanup that gives real sats back (spec §5).
async fn capture_failure(def: &ScenarioDef, ctx: &Context) {
    let name = format!("{}-failed", def.name);
    let captures = ctx.actors.iter().map(|session| {
        let name = name.as_str();
        async move {
            let step = tokio::time::timeout(
                FAILURE_CAPTURE_DEADLINE,
                ctx.evidence
                    .checkpoint(session.actor.as_str(), name, session.ui.as_ref()),
            )
            .await;
            (session.actor, step)
        }
    });
    // Pushed in actor order once both are done, so the timeline reads the
    // same whichever phone answered first.
    for (actor, step) in futures::future::join_all(captures).await {
        match step {
            Ok(step) => ctx.timeline.push(
                TimelineKind::Step,
                Some(actor.as_str()),
                format!("screen when {} failed", def.name),
                Some(step),
            ),
            Err(_) => warn!(
                scenario = def.name,
                actor = actor.as_str(),
                "no screen recorded: the failure capture hit its deadline"
            ),
        }
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
        // Orders reopened from a recovery journal were never scoped into
        // this observer; the relay confirmation below needs them to be.
        if let Err(e) = ctx.relay.add_order(&entry.order.id).await {
            warn!(order = %entry.order.id, error = %e, "cannot scope observer to order");
        }
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
    if ctx.journal.enabled()
        && let Ok(session) = ctx.actor(maker)
        && ctx
            .adapter
            .observe_order(&session.device(ctx.ui_timeout), order)
            .await
            .is_ok_and(|state| state.state == OrderState::PayoutPending)
    {
        return match mortsom_scenario_kit::actions::wait_app_state(
            ctx,
            maker,
            order,
            OrderState::Completed,
        )
        .await
        {
            Ok(_) => CleanupResult::Done {
                outcome: CancelOutcome::AlreadyClosed,
            },
            Err(error) => CleanupResult::Failed {
                message: error.to_string(),
            },
        };
    }
    let outcome = match cancel_as(ctx, maker, order).await {
        Ok(CancelOutcome::NotFound) => return confirm_unlisted_closed(ctx, order).await,
        Ok(outcome) => outcome,
        Err(message) => return CleanupResult::Failed { message },
    };
    // The maker's request alone closes an unfunded order; a funded one
    // needs the counterpart's agreement. The maker's screen cannot tell
    // the two apart: app v2 marks the order cancelled on the request itself,
    // so a "closed" label is trusted only once the relay carries it.
    match observed_state(ctx, maker, order).await {
        Some(state) if state.is_terminal() => {
            match confirm_closed(ctx, order, state, outcome).await {
                CleanupResult::Failed { .. } if state == OrderState::Cancelled => {}
                confirmed => return confirmed,
            }
        }
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
        Some(state) if state.is_terminal() => confirm_closed(ctx, order, state, outcome).await,
        Some(state) => CleanupResult::Failed {
            message: format!("still {state} after both sides cancelled"),
        },
        None => CleanupResult::Failed {
            message: "state unreadable after both sides cancelled".into(),
        },
    }
}

/// An order the app no longer lists is done only when the daemon says it is
/// closed on the relay (`canceled`, `success` or `expired`). A client that
/// cannot show a trade it created has not thereby cancelled it: the order
/// may still sit pending on the daemon, waiting for a taker.
async fn confirm_unlisted_closed(ctx: &Context, order: &OrderReference) -> CleanupResult {
    let id = order.id.clone();
    let closed = ctx
        .relay
        .wait_for(ctx.state_timeout, |e| {
            e.kind == mortsom_scenario_kit::actions::ORDER_EVENT_KIND
                && e.tag("d") == Some(id.as_str())
                && e.tag("s")
                    .and_then(OrderState::from_protocol_status)
                    .is_some_and(|state| state.is_terminal())
        })
        .await;
    match closed {
        Some(event) => {
            ctx.timeline.push_at(
                event.received_at_ms,
                TimelineKind::RelayEvent,
                None,
                format!(
                    "relay: order {} is closed ({}) although the app lists no trade for it",
                    order.id,
                    event.tag("s").unwrap_or("?")
                ),
                Some(event.id.clone()),
            );
            CleanupResult::Done {
                outcome: CancelOutcome::NotFound,
            }
        }
        None => CleanupResult::Failed {
            message: format!(
                "the app lists no trade for order {} and the relay never showed it closed",
                order.id
            ),
        },
    }
}

/// The daemon closes an order, not a screen. A `cancelled` label is accepted
/// only once the relay carries the daemon's own `canceled` status for that
/// order: a funded order whose maker asked to cancel shows the request in the
/// UI while the daemon still holds the escrow and waits for the counterpart.
async fn confirm_closed(
    ctx: &Context,
    order: &OrderReference,
    state: OrderState,
    outcome: CancelOutcome,
) -> CleanupResult {
    if state != OrderState::Cancelled {
        return CleanupResult::Done { outcome };
    }
    match mortsom_scenario_kit::actions::wait_relay_state(ctx, order, OrderState::Cancelled).await {
        Ok(()) => CleanupResult::Done { outcome },
        Err(_) => CleanupResult::Failed {
            message: format!(
                "the UI shows order {} cancelled but the relay never published canceled for it",
                order.id
            ),
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
