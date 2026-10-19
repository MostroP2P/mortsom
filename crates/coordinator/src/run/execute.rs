//! Scenario execution: timeout, panic isolation, the single infrastructure
//! retry (spec §10), and the cleanup sweep after each scenario.

use std::panic::AssertUnwindSafe;
use std::time::{Duration, Instant};

use futures::FutureExt;
use mortsom_app_contract::{CancelOutcome, OrderReference, OrderState};
use mortsom_common::Actor;
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

/// Runs `def` against `ctx` exactly once. A client keeps its actor storage
/// for the whole run, so a second attempt would reopen the trade the first
/// one started rather than begin a fresh one; an infrastructure failure is
/// reported as such and the operator reruns the scenario (spec §10).
pub async fn execute_scenario(
    def: &ScenarioDef,
    ctx: &Context,
    app_profile: &str,
    app_version: Option<String>,
) -> ScenarioResult {
    let attempts = 1;
    let started = Instant::now();
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
    info!(scenario = def.name, attempts, outcome = %describe(&outcome), "scenario finished");
    ScenarioResult {
        name: def.name.into(),
        app_profile: app_profile.into(),
        app_version,
        outcome,
        attempts,
        duration,
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
    if let Some(settled) = settled_payout(ctx, maker, order).await {
        return settled;
    }
    // A disputed order is out of the parties' hands: no cancel from either
    // app closes it. The run's solver, when there is one, rules on it the
    // way an operator would after a broken run -- returning the escrow.
    if ctx.solver.is_some() && shown_disputed(ctx, maker, order).await {
        return resolve_by_solver(ctx, order).await;
    }
    let outcome = match cancel_as(ctx, maker, order).await {
        Ok(CancelOutcome::NotFound) => return confirm_closed_on_relay(ctx, order, UNLISTED).await,
        Ok(outcome) => outcome,
        Err(message) => return CleanupResult::Failed { message },
    };
    if let Some(settled) = settled_after_maker_cancel(ctx, maker, order, outcome).await {
        return settled;
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
        Shown::State(state) if state.is_terminal() => {
            confirm_closed(ctx, order, state, outcome).await
        }
        Shown::State(state) => CleanupResult::Failed {
            message: format!("still {state} after both sides cancelled"),
        },
        Shown::Unlisted => confirm_closed_on_relay(ctx, order, UNLISTED).await,
        Shown::Unreadable => confirm_closed_on_relay(ctx, order, UNREADABLE).await,
    }
}

/// A recovered order that was already paying out when the run stopped
/// settles by itself: the daemon's success closes it, not a cancel.
/// `None` when the order is not at payout.
async fn settled_payout(
    ctx: &Context,
    maker: Actor,
    order: &OrderReference,
) -> Option<CleanupResult> {
    if ctx.journal.enabled()
        && let Ok(session) = ctx.actor(maker)
        && ctx
            .adapter
            .observe_order(&session.device(ctx.ui_timeout), order)
            .await
            .is_ok_and(|state| state.state == OrderState::PayoutPending)
    {
        let shown =
            mortsom_scenario_kit::actions::wait_app_state(ctx, maker, order, OrderState::Completed)
                .await;
        return Some(match shown {
            Ok(_) => CleanupResult::Done {
                outcome: CancelOutcome::AlreadyClosed,
            },
            // A seller client that missed the public success event keeps
            // showing payout pending; the daemon's own success closes the
            // order, and the settlement records are verified afterwards.
            Err(lagging) => match mortsom_scenario_kit::actions::wait_relay_state(
                ctx,
                order,
                OrderState::Completed,
            )
            .await
            {
                Ok(()) => {
                    ctx.timeline.push(
                        TimelineKind::Cleanup,
                        Some(maker.as_str()),
                        format!(
                            "order {}: screen stayed short of completed ({lagging}); the daemon published success",
                            order.id
                        ),
                        None,
                    );
                    CleanupResult::Done {
                        outcome: CancelOutcome::AlreadyClosed,
                    }
                }
                Err(_) => CleanupResult::Failed {
                    message: lagging.to_string(),
                },
            },
        });
    }
    None
}

/// What the maker's cancel alone settled, or `None` when the counterpart's
/// agreement is still wanted.
///
/// The maker's request alone closes an unfunded order; a funded one needs
/// the counterpart's agreement. The maker's screen cannot tell the two
/// apart: app v2 marks the order cancelled on the request itself, so a
/// "closed" label is trusted only once the relay carries it.
async fn settled_after_maker_cancel(
    ctx: &Context,
    maker: Actor,
    order: &OrderReference,
    outcome: CancelOutcome,
) -> Option<CleanupResult> {
    match observed_state(ctx, maker, order).await {
        Shown::State(state) if state.is_terminal() => {
            match confirm_closed(ctx, order, state, outcome).await {
                CleanupResult::Failed { .. } if state == OrderState::Cancelled => None,
                confirmed => Some(confirmed),
            }
        }
        Shown::State(OrderState::Active | OrderState::FiatSent) => None,
        Shown::State(state) => Some(CleanupResult::Failed {
            message: format!("still {state} after the maker cancelled"),
        }),
        // App v2 drops a cancelled trade that never went active from My
        // Trades, and its state goes with it. Once the maker's loaded list
        // no longer shows it, only the relay can say the daemon closed it.
        Shown::Unlisted => Some(confirm_closed_on_relay(ctx, order, UNLISTED).await),
        // The maker's screen cannot be read (live, 2026-10-15: her trade
        // detail lost `order.id` once she asked to cancel). The relay still
        // says whether the daemon holds the order: closed is confirmed
        // there; open means the counterpart's agreement is still wanted.
        Shown::Unreadable => match relay_state(ctx, order) {
            Some(state) if state.is_terminal() => {
                Some(confirm_closed_on_relay(ctx, order, UNREADABLE).await)
            }
            Some(_) => None,
            None => Some(CleanupResult::Failed {
                message: "state unreadable after the maker cancelled".into(),
            }),
        },
    }
}

/// Why the relay, not a screen, has to say the order is closed.
const UNLISTED: &str = "the app lists no trade for it";
const UNREADABLE: &str = "the maker's app cannot show its state";

/// The daemon's latest published status for `order`, as far as the run's
/// observer has seen it; `None` before the first order event.
fn relay_state(ctx: &Context, order: &OrderReference) -> Option<OrderState> {
    ctx.relay
        .snapshot()
        .into_iter()
        .rev()
        .filter(|e| {
            e.kind == mortsom_scenario_kit::actions::ORDER_EVENT_KIND
                && e.tag("d") == Some(order.id.as_str())
        })
        .find_map(|e| e.tag("s").and_then(OrderState::from_protocol_status))
}

/// Whether `actor`'s app shows `order` disputed right now.
async fn shown_disputed(ctx: &Context, actor: Actor, order: &OrderReference) -> bool {
    let Ok(session) = ctx.actor(actor) else {
        return false;
    };
    ctx.adapter
        .observe_order(&session.device(ctx.ui_timeout), order)
        .await
        .is_ok_and(|state| state.state == OrderState::Disputed)
}

/// The solver takes each open dispute the daemon published during the run
/// until one turns out to be about `order`, then cancels the order so the
/// escrow goes back to the seller. The dispute event never names its
/// order; the daemon's answer to the take does.
async fn resolve_by_solver(ctx: &Context, order: &OrderReference) -> CleanupResult {
    let Some(solver) = ctx.solver.as_ref() else {
        return CleanupResult::Failed {
            message: "disputed, and this run has no solver".into(),
        };
    };
    let open = [
        mortsom_scenario_kit::actions::dispute_status::INITIATED,
        mortsom_scenario_kit::actions::dispute_status::IN_PROGRESS,
    ];
    let mut disputes: Vec<String> = Vec::new();
    for event in ctx.relay.snapshot().into_iter().rev() {
        if event.kind == mortsom_scenario_kit::actions::DISPUTE_EVENT_KIND
            && event.tag("s").is_some_and(|s| open.contains(&s))
            && let Some(id) = event.tag("d")
            && !disputes.iter().any(|known| known == id)
        {
            disputes.push(id.to_owned());
        }
    }
    for dispute in &disputes {
        match solver.take_dispute(dispute).await {
            Ok(about) if about == order.id => {
                if let Err(e) = solver.cancel(&order.id).await {
                    return CleanupResult::Failed {
                        message: format!("the solver could not cancel the disputed order: {e}"),
                    };
                }
                ctx.timeline.push(
                    TimelineKind::Cleanup,
                    Some("solver"),
                    format!("order {}: the solver cancelled the dispute", order.id),
                    None,
                );
                return match mortsom_scenario_kit::actions::wait_relay_state(
                    ctx,
                    order,
                    OrderState::Cancelled,
                )
                .await
                {
                    Ok(()) => CleanupResult::Done {
                        outcome: CancelOutcome::Cancelled,
                    },
                    Err(e) => CleanupResult::Failed {
                        message: e.to_string(),
                    },
                };
            }
            Ok(_) => {}
            Err(e) => warn!(dispute, error = %e, "the solver could not take a dispute"),
        }
    }
    CleanupResult::Failed {
        message: format!(
            "disputed, and none of the {} open disputes the daemon published is about it",
            disputes.len()
        ),
    }
}

/// An order whose state no screen can show is done only when the daemon
/// says it is closed on the relay (`canceled`, `success` or `expired`). A
/// client that cannot show a trade it created has not thereby cancelled it:
/// the order may still sit pending on the daemon, waiting for a taker, or
/// funded, waiting for the counterpart. `why` names what the screen lacks.
async fn confirm_closed_on_relay(
    ctx: &Context,
    order: &OrderReference,
    why: &str,
) -> CleanupResult {
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
                    "relay: order {} is closed ({}) although {why}",
                    order.id,
                    event.tag("s").unwrap_or("?")
                ),
                Some(event.id.clone()),
            );
            // The daemon closed it: that is what "already closed" means.
            CleanupResult::Done {
                outcome: CancelOutcome::AlreadyClosed,
            }
        }
        None => CleanupResult::Failed {
            message: format!(
                "order {}: {why} and the relay never showed it closed",
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

/// What an actor's app shows of an order.
enum Shown {
    /// The state on its screen.
    State(OrderState),
    /// The loaded My Trades no longer lists it (spec §8.3).
    Unlisted,
    /// Neither its state nor the list could be read.
    Unreadable,
}

/// What `actor`'s app shows of `order` once it stops changing, bounded by
/// the state timeout. A trade the loaded list no longer shows ends the wait
/// at once: there is no row left to change, and reading through one that is
/// gone only waits out the UI timeout (live, 2026-10-19, #93).
async fn observed_state(ctx: &Context, actor: Actor, order: &OrderReference) -> Shown {
    let Ok(session) = ctx.actor(actor) else {
        return Shown::Unreadable;
    };
    let device = session.device(ctx.ui_timeout);
    let read = || async {
        match ctx.adapter.observe_listed_order(&device, order).await {
            Ok(Some(shown)) => Shown::State(shown.state),
            Ok(None) => Shown::Unlisted,
            Err(_) => Shown::Unreadable,
        }
    };
    let settled = wait_until(ctx.state_timeout, CLEANUP_POLL, || async {
        match read().await {
            Shown::State(state) if !state.is_terminal() => None,
            Shown::Unreadable => None,
            settled => Some(settled),
        }
    })
    .await;
    match settled {
        Some(settled) => settled,
        None => read().await,
    }
}
