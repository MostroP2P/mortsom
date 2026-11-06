//! Payment-failure family (spec §8, tag `payment-failure`): payouts the
//! daemon cannot pay, how it recovers, and how many unresolved payouts it
//! lets one payee pin. The buyer's node is Mortsom's, so the invoices are
//! shaped from the outside -- withdrawn before the release, too short to
//! be accepted, or held once paid -- and the proof is what the daemon does
//! next: the retries it declares, the replacement it asks for, the payout
//! it refuses to dispatch, and where every sat ends up.

use mortsom_app_contract::{Capability, OrderReference, OrderState};
use mortsom_common::Actor;
use mortsom_scenario_kit::actions::{self, BuyerInvoice, Refusal};
use mortsom_scenario_kit::context::{Context, HoldInvoiceState, ScenarioError};

use crate::flow;
use crate::recovery::TRADE_TIMEOUT_SECS;

/// Deadline for the scenario that drives several trades and waits for
/// the daemon's payout reconciliation.
pub const CAPPED_PAYOUTS_TIMEOUT_SECS: u64 = 40 * 60;

/// Capabilities needed to fund a trade and see its payout replaced.
pub const REPLACEMENT_REQUIRES: &[Capability] = flow::TRADE_REQUIRES;

/// The labels the buyer's app shows while the daemon waits for an invoice.
const WAITING_INVOICE: &[&str] = &["waiting-invoice", "waiting-buyer-invoice"];

fn payout_hash(taken: &flow::Taken) -> Result<&str, ScenarioError> {
    taken
        .payout_hash
        .as_deref()
        .ok_or_else(|| ScenarioError::wallet("buyer payout hash is required for this scenario"))
}

/// The buyer's invoice is gone by the time the seller releases: Bob
/// withdraws it on his node after reporting fiat sent. The daemon settles
/// the escrow anyway, fails to pay, retries as it declares, then asks Bob
/// for a new invoice; Bob hands one in and the trade completes on it. The
/// hold settled once, the replacement settled once, and the withdrawn
/// invoice never did.
pub async fn payout_fails_then_replaced(ctx: &Context) -> Result<(), ScenarioError> {
    let taken = flow::create_take_and_fund(ctx).await?;
    flow::fiat_sent(ctx, &taken.order).await?;
    let withdrawn = payout_hash(&taken)?.to_owned();
    actions::supersede_buyer_invoice(ctx, Actor::Bob, &taken.order, &withdrawn).await?;
    let since = actions::relay_cursor(ctx);
    actions::release(ctx, Actor::Alice, &taken.order).await?;
    actions::settle_hold_payment(ctx, &taken.order).await?;
    actions::wait_app_label(ctx, Actor::Bob, &taken.order, WAITING_INVOICE).await?;
    let replacement = actions::provide_buyer_invoice(ctx, Actor::Bob, &taken.order)
        .await?
        .ok_or_else(|| ScenarioError::wallet("the replacement payout hash is required"))?;
    actions::wait_both(ctx, &taken.order, OrderState::Completed).await?;
    actions::wait_relay_state_since(
        ctx,
        &taken.order,
        &[OrderState::Completed],
        since,
        ctx.state_timeout,
    )
    .await?;
    actions::expect_payout_settled(ctx, Actor::Bob, &taken.order, &replacement).await?;
    actions::expect_payout_never_settled(ctx, Actor::Bob, &taken.order, &withdrawn).await?;
    actions::mark_resolved(ctx, &taken.order);
    Ok(())
}

/// The app's word for an invoice that expires within the daemon's window.
const EXPIRES_TOO_SOON: &str = "expires-too-soon";
/// The app's word for an invoice it lets through.
const VALID: &str = "valid";

/// Bob hands in an invoice that expires before the daemon's declared
/// floor. It is refused: by the app, which never sends it once the node's
/// capabilities have reached it, or before that by the daemon. Either way
/// the order stays taken and waiting for an invoice; a proper one, which
/// the app calls valid, is then accepted. No value moves; the cleanup
/// sweep closes the order.
pub async fn payout_invoice_below_expiry_floor(ctx: &Context) -> Result<(), ScenarioError> {
    let floor = ctx
        .limits
        .payout_invoice_min_expiry_secs
        .filter(|floor| *floor > 1)
        .ok_or_else(|| {
            ScenarioError::environment(
                "the stack declares no payout invoice expiry floor (payout_invoice_min_expiry_secs)",
            )
        })?;
    let order = flow::create_pending(ctx).await?;
    actions::take_order(ctx, Actor::Bob, &order).await?;
    actions::wait_both(ctx, &order, OrderState::Taken).await?;
    let refused = actions::submit_rejected_buyer_invoice(
        ctx,
        Actor::Bob,
        &order,
        BuyerInvoice::Expiring { secs: floor / 2 },
    )
    .await?;
    // A client that judges invoices itself names its verdict; one that
    // does not (the CLI) sends the invoice and the daemon is the judge.
    let judges = ctx
        .adapter
        .capabilities()
        .supports(Capability::InvoiceCheck);
    if let Refusal::App { check } = &refused.refusal
        && (!judges || check != EXPIRES_TOO_SOON)
    {
        return Err(ScenarioError::product(format!(
            "the client refused an invoice below the expiry floor as `{check}`, not `{EXPIRES_TOO_SOON}`"
        )));
    }
    // Still taken, still waiting for Bob's invoice, on both sides.
    actions::wait_app_label(ctx, Actor::Bob, &order, WAITING_INVOICE).await?;
    actions::wait_app_state(ctx, Actor::Alice, &order, OrderState::Taken).await?;
    let accepted =
        actions::submit_accepted_buyer_invoice(ctx, Actor::Bob, &order, BuyerInvoice::Standard)
            .await?;
    let expected = judges.then_some(VALID);
    if accepted.check.as_deref() != expected {
        return Err(ScenarioError::product(format!(
            "the client sent a proper invoice judging it {:?}, not {expected:?}",
            accepted.check
        )));
    }
    actions::wait_app_state(ctx, Actor::Bob, &order, OrderState::Taken).await?;
    actions::wait_app_state(ctx, Actor::Alice, &order, OrderState::Taken).await?;
    actions::wait_relay_state(ctx, &order, OrderState::Taken).await?;
    actions::expect_payout_never_settled(ctx, Actor::Bob, &order, &refused.hash).await?;
    Ok(())
}

/// One funded trade whose buyer invoice is a hold invoice on Bob's node.
async fn fund_with_hold_invoice(ctx: &Context) -> Result<(OrderReference, String), ScenarioError> {
    let order = flow::create_pending(ctx).await?;
    actions::take_order(ctx, Actor::Bob, &order).await?;
    actions::wait_both(ctx, &order, OrderState::Taken).await?;
    let hash = actions::provide_buyer_invoice_of(ctx, Actor::Bob, &order, BuyerInvoice::Hold)
        .await?
        .ok_or_else(|| ScenarioError::wallet("the hold invoice hash is required"))?;
    actions::wait_app_state(ctx, Actor::Alice, &order, OrderState::Taken).await?;
    flow::fund(ctx, &order).await?;
    flow::fiat_sent(ctx, &order).await?;
    Ok((order, hash))
}

/// Bob's payout invoices are hold invoices he never settles: each payout
/// the daemon dispatches stays in flight on his node. Up to the declared
/// per-destination cap the daemon dispatches; the next release settles
/// the escrow but sends nothing to Bob until he settles one of the held
/// payouts, after which the held payout goes out on its own. Every trade
/// completes once Bob settles them all.
pub async fn payout_slots_capped_per_destination(ctx: &Context) -> Result<(), ScenarioError> {
    let cap = ctx
        .limits
        .max_inflight_payouts_per_destination
        .filter(|cap| *cap > 0)
        .ok_or_else(|| {
            ScenarioError::environment(
                "the stack declares no per-destination payout cap (max_inflight_payouts_per_destination)",
            )
        })?;
    let cap = usize::try_from(cap).map_err(|_| ScenarioError::environment("cap out of range"))?;
    let mut trades = Vec::with_capacity(cap + 1);
    for _ in 0..=cap {
        trades.push(fund_with_hold_invoice(ctx).await?);
    }
    let outcome = drive_capped_payouts(ctx, cap, &trades).await;
    if outcome.is_err() {
        // Whatever failed, no payout stays pinned on Bob's node behind it.
        let problems = actions::release_held_payouts(ctx, Actor::Bob, &trades).await;
        for problem in problems {
            ctx.timeline.push(
                mortsom_scenario_kit::timeline::TimelineKind::Cleanup,
                Some(Actor::Bob.as_str()),
                format!("held payout not released: {problem}"),
                None,
            );
        }
    }
    outcome
}

/// The releases, the cap taking effect, and every payout landing once Bob
/// settles what he holds.
async fn drive_capped_payouts(
    ctx: &Context,
    cap: usize,
    trades: &[(OrderReference, String)],
) -> Result<(), ScenarioError> {
    let since = actions::relay_cursor(ctx);
    release_all_holding_up_to(ctx, cap, trades).await?;
    prove_the_capped_payout_waits(ctx, cap, trades).await?;
    settle_all_and_complete(ctx, trades, since).await
}

/// Releases every trade: the escrow settles once per release, and up to
/// the cap each payout lands on Bob's node as an accepted hold invoice.
async fn release_all_holding_up_to(
    ctx: &Context,
    cap: usize,
    trades: &[(OrderReference, String)],
) -> Result<(), ScenarioError> {
    for (index, (order, hash)) in trades.iter().enumerate() {
        actions::release(ctx, Actor::Alice, order).await?;
        actions::settle_hold_payment(ctx, order).await?;
        if index < cap {
            actions::wait_hold_invoice(
                ctx,
                Actor::Bob,
                order,
                hash,
                HoldInvoiceState::Accepted,
                ctx.settle_timeout,
            )
            .await?;
        }
    }
    Ok(())
}

/// The trade past the cap proves the reconciliation: nothing is dispatched
/// to it for the whole quiet window, and once Bob settles one held payout
/// the waiting one goes out on its own.
async fn prove_the_capped_payout_waits(
    ctx: &Context,
    cap: usize,
    trades: &[(OrderReference, String)],
) -> Result<(), ScenarioError> {
    let (pinned_order, pinned_hash) = &trades[cap];
    actions::expect_hold_invoice_stays(
        ctx,
        Actor::Bob,
        pinned_order,
        pinned_hash,
        HoldInvoiceState::Open,
        ctx.recovery.quiet_window,
    )
    .await?;
    let (first_order, first_hash) = &trades[0];
    actions::settle_buyer_hold_invoice(ctx, Actor::Bob, first_order, first_hash).await?;
    actions::expect_payout_settled(ctx, Actor::Bob, first_order, first_hash).await?;
    actions::wait_hold_invoice(
        ctx,
        Actor::Bob,
        pinned_order,
        pinned_hash,
        HoldInvoiceState::Accepted,
        ctx.expiration_timeout,
    )
    .await
}

/// Bob settles the rest; every payout lands once and every trade completes
/// on both apps and on the book.
async fn settle_all_and_complete(
    ctx: &Context,
    trades: &[(OrderReference, String)],
    since: u64,
) -> Result<(), ScenarioError> {
    for (order, hash) in trades.iter().skip(1) {
        actions::settle_buyer_hold_invoice(ctx, Actor::Bob, order, hash).await?;
        actions::expect_payout_settled(ctx, Actor::Bob, order, hash).await?;
    }
    for (order, _) in trades {
        for actor in Actor::ALL {
            actions::wait_app_state_in(
                ctx,
                actor,
                order,
                &[OrderState::Completed],
                ctx.expiration_timeout,
            )
            .await?;
        }
        actions::wait_relay_state_since(
            ctx,
            order,
            &[OrderState::Completed],
            since,
            ctx.expiration_timeout,
        )
        .await?;
        actions::mark_resolved(ctx, order);
    }
    Ok(())
}

crate::register_scenario! {
    name: "payout_fails_then_replaced",
    body: payout_fails_then_replaced,
    tags: &["payment-failure"],
    requires: REPLACEMENT_REQUIRES,
    timeout_secs: TRADE_TIMEOUT_SECS,
    moves_value: true,
}
crate::register_scenario! {
    name: "payout_invoice_below_expiry_floor",
    body: payout_invoice_below_expiry_floor,
    tags: &["payment-failure"],
    requires: flow::TAKE_REQUIRES,
    timeout_secs: TRADE_TIMEOUT_SECS,
    moves_value: false,
}
crate::register_scenario! {
    name: "payout_slots_capped_per_destination",
    body: payout_slots_capped_per_destination,
    tags: &["payment-failure"],
    requires: REPLACEMENT_REQUIRES,
    timeout_secs: CAPPED_PAYOUTS_TIMEOUT_SECS,
    moves_value: true,
}

/// Keeps registrations linked.
pub fn link() {}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
