//! The taker's anti-abuse bond (spec §8.7): paying the bond the daemon
//! asks for before the trade starts, walking away from it, and proving
//! where its sats ended up. On every honest exit the daemon cancels the
//! HTLC it held, so the bond comes back; a bond that settled was slashed.
use super::*;
use crate::context::{BondPolicy, HoldInvoiceState, NodeWallet};

/// The pending-hold slot of `actor`'s bond on an order, next to the
/// order's own escrow and the other side's bond: a seller who takes a buy
/// pays a bond and the escrow, and a bonded maker holds theirs alongside.
fn bond_key(actor: Actor, order: &OrderReference) -> String {
    format!("{}#bond:{actor}", order.id)
}

/// The bond policy the stack declares, or the environment error a bond
/// scenario ends with before an order exists when the stack declares
/// none: without `stack up --bond` the daemon asks for no bond, and a
/// scenario about one would be waiting for a screen that never opens.
pub fn bond_policy(ctx: &Context) -> Result<BondPolicy, ScenarioError> {
    ctx.limits.bond.ok_or_else(|| {
        ScenarioError::environment(
            "the stack declares no anti-abuse bond; run `mortsom stack up --bond` for the bond family",
        )
    })
}

/// The environment error a bonded trade ends with, before any wallet
/// action, when one order cannot hold every bond the policy asks for and
/// the escrow together within the per-order cap: the maker's bond, the
/// taker's and the escrow all sit on one order, and the cap bounds every
/// reservation an order holds (spec 10). Found only at the escrow, it
/// would strand locked bonds on a half-funded order.
pub fn require_room_for_bond_and_escrow(ctx: &Context) -> Result<(), ScenarioError> {
    let policy = bond_policy(ctx)?;
    let bond = policy.bond_sats(ctx.amount_sats);
    let bonds = u64::from(policy.maker_posts()) + u64::from(policy.apply_to.taker_posts());
    let fee_cap = [Actor::Alice, Actor::Bob]
        .into_iter()
        .filter_map(|actor| ctx.node_wallet(actor))
        .map(|node| node.routing_fee_cap_sats())
        .max()
        .unwrap_or_default();
    // Each bond and the escrow is a payment with its own fee ceiling.
    let needed = ctx
        .amount_sats
        .saturating_add(bond.saturating_mul(bonds))
        .saturating_add(fee_cap.saturating_mul(bonds + 1));
    let cap = ctx
        .budget
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .caps()
        .max_order_sats;
    if needed > cap {
        return Err(ScenarioError::environment(format!(
            "{bonds} bond(s) of {bond} sats and the {} sat escrow need {needed} sats on one order with \
             their fee ceilings, over run.max_order_sats {cap}",
            ctx.amount_sats
        )));
    }
    Ok(())
}

/// Pays the anti-abuse bond as `actor`, from the node Mortsom holds for
/// them, within the caps. The app is left on the pay-bond screen and the
/// bolt11 it shows is paid from outside; the daemon holds the HTLC until
/// the trade ends, so like the escrow this only confirms the payment is
/// in flight. [`expect_bond_released`] resolves it. The invoice must ask
/// exactly what the declared policy sizes the bond at for this order.
pub async fn pay_bond(
    ctx: &Context,
    actor: Actor,
    order: &OrderReference,
) -> Result<PaymentObservation, ScenarioError> {
    ctx.require_mainnet()?;
    let expected = bond_policy(ctx)?.bond_sats(ctx.amount_sats);
    let node = ctx.node_wallet(actor).ok_or_else(|| {
        ScenarioError::environment(format!(
            "the bond is paid from a node Mortsom holds, and none is configured for {actor}"
        ))
    })?;
    let session = ctx.actor(actor)?;
    let mut prepared = hold_budget::PreparedReservation::reserve(ctx, &order.id, expected)?;
    let handoff = ctx
        .adapter
        .pay_bond_invoice(
            &session.device(ctx.ui_timeout),
            order,
            PaymentPath::ExternalWallet,
        )
        .await?;
    if handoff.kind != PaymentPath::ExternalWallet {
        return Err(ScenarioError::product(
            "the bond used a different payment path than requested",
        ));
    }
    let Some(invoice) = handoff.invoice.as_ref() else {
        return Err(ScenarioError::product(
            "the app did not expose the bond invoice; the payment cannot be correlated",
        ));
    };
    ctx.protect(invoice);
    let hash = payment_hash_of(invoice).map_err(|e| ScenarioError::product(e.to_string()))?;
    let amount_sats = prepared.requote(invoice, node.routing_fee_cap_sats())?;
    if amount_sats != expected {
        return Err(ScenarioError::product(format!(
            "the bond invoice for order {} asks {amount_sats} sats; the declared policy sizes the bond at {expected}",
            order.id
        )));
    }
    ctx.journal
        .record(RecoveryEvent::PaymentExpected {
            order_id: order.id.clone(),
            wallet: actor.as_str().into(),
            hash: hash.clone(),
            incoming: false,
            amount_sats,
            bond: true,
        })
        .map_err(ScenarioError::environment)?;
    prepared.payment_may_start(Some(hash.clone()))?;
    node.start_payment(invoice)
        .await
        .map_err(ScenarioError::environment)?;
    let reservation = prepared.into_reservation();
    let step = ctx
        .evidence
        .checkpoint(actor.as_str(), "pay-bond", session.ui.as_ref())
        .await;
    ctx.timeline.push(
        TimelineKind::Step,
        Some(actor.as_str()),
        format!(
            "paid the {amount_sats} sat bond for order {} from the node (hash {})",
            order.id,
            short(&hash)
        ),
        Some(step),
    );
    hold::accept_in_flight(
        ctx,
        actor,
        order,
        hold::InFlight {
            key: &bond_key(actor, order),
            wallet_label: &session.wallet_label,
            reservation,
            hash,
            what: "bond",
            then: "released when the trade ends",
        },
    )
    .await
}

/// Walks away from the take on the pay-bond screen as `actor`: nothing
/// was paid, and the order stays in the book for others.
pub async fn decline_bond(
    ctx: &Context,
    actor: Actor,
    order: &OrderReference,
) -> Result<(), ScenarioError> {
    let session = ctx.actor(actor)?;
    ctx.adapter
        .decline_bond(&session.device(ctx.ui_timeout), order)
        .await?;
    let step = ctx
        .evidence
        .checkpoint(actor.as_str(), "decline-bond", session.ui.as_ref())
        .await;
    ctx.timeline.push(
        TimelineKind::Step,
        Some(actor.as_str()),
        format!("declined the bond of order {}", order.id),
        Some(step),
    );
    Ok(())
}

/// `actor`'s bond came back: after an honest exit the daemon cancels the
/// HTLC it held, so the payment must fail on their node and the
/// reservation is freed. A bond that settled instead was slashed, which
/// no honest exit allows. No-op when `actor` has no bond in flight on the
/// order.
pub async fn expect_bond_released(
    ctx: &Context,
    actor: Actor,
    order: &OrderReference,
) -> Result<(), ScenarioError> {
    hold::expect_returned(
        ctx,
        order,
        hold::Returned {
            key: &bond_key(actor, order),
            returned: "bond of order",
            kept: "the bond was slashed on an honest exit",
        },
    )
    .await
}

/// `actor`'s bond was slashed: the daemon settled the HTLC it held, so
/// the payment must settle on their node and the budget records the
/// spend. A bond that came back instead was released, which no slash
/// allows.
pub async fn expect_bond_slashed(
    ctx: &Context,
    actor: Actor,
    order: &OrderReference,
) -> Result<(), ScenarioError> {
    let settled = hold::expect_settled(ctx, order, &bond_key(actor, order), "bond").await?;
    match settled {
        Some(obs) => {
            ctx.timeline.push(
                TimelineKind::Assertion,
                Some(actor.as_str()),
                format!(
                    "the {} sat bond of order {} was slashed: settled on {actor}'s node",
                    obs.amount_sats, order.id
                ),
                None,
            );
            Ok(())
        }
        None => Err(ScenarioError::environment(format!(
            "{actor} has no bond in flight on order {}",
            order.id
        ))),
    }
}

/// Waits until `actor`'s app offers a claim for `order` and returns it as
/// the app reads it; the phase is the caller's to judge.
pub async fn wait_bond_claim(
    ctx: &Context,
    actor: Actor,
    order: &OrderReference,
) -> Result<mortsom_app_contract::BondClaim, ScenarioError> {
    let session = ctx.actor(actor)?;
    let device = session.device(ctx.ui_timeout);
    let found = wait_until(ctx.state_timeout, STATE_POLL, || async {
        match ctx.adapter.bond_claim(&device, order).await {
            Ok(Some(claim)) => Some(claim),
            _ => None,
        }
    })
    .await;
    let claim = found.ok_or_else(|| {
        ScenarioError::product(format!(
            "{actor}'s app offered no claim on order {} within {:?}",
            order.id, ctx.state_timeout
        ))
    })?;
    ctx.timeline.push(
        TimelineKind::UiState,
        Some(actor.as_str()),
        format!(
            "the app offers {actor} a claim of {} sats on order {} ({})",
            claim.amount_sats, order.id, claim.status
        ),
        None,
    );
    Ok(claim)
}

/// Waits until the claim `actor`'s app shows for `order` reads one of
/// `phases`.
async fn wait_bond_claim_phase(
    ctx: &Context,
    actor: Actor,
    order: &OrderReference,
    phases: &[&str],
) -> Result<String, ScenarioError> {
    let session = ctx.actor(actor)?;
    let device = session.device(ctx.ui_timeout);
    let found = wait_until(ctx.state_timeout, STATE_POLL, || async {
        match ctx.adapter.bond_claim(&device, order).await {
            Ok(Some(claim)) if phases.contains(&claim.status.as_str()) => Some(claim.status),
            _ => None,
        }
    })
    .await;
    found.ok_or_else(|| {
        ScenarioError::product(format!(
            "{actor}'s claim on order {} did not read {} within {:?}",
            order.id,
            phases.join("|"),
            ctx.state_timeout
        ))
    })
}

/// The phases a claim passes through once its invoice is in.
const CLAIM_IN_PROGRESS: &[&str] = &["submitted", "acknowledged", "completed"];
/// The phase of a paid claim.
const CLAIM_COMPLETED: &[&str] = &["completed"];
/// The phase of a claim the daemon is asking an invoice for.
const CLAIM_PENDING: &[&str] = &["pending"];

/// The invoice a winner hands in for a claim.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ClaimInvoice {
    /// An ordinary invoice, which the daemon pays and the node settles.
    Plain,
    /// A hold invoice: the daemon's payment is held on the winner's node
    /// until the node settles or cancels it.
    Hold,
}

/// `actor` claims their share of a slashed bond on `order`: the app must
/// offer exactly what the declared policy leaves the winner, an invoice
/// for it is issued on their node and handed in, the daemon pays it, and
/// the app reads the claim completed. Returns the payout hash.
pub async fn claim_bond_payout(
    ctx: &Context,
    actor: Actor,
    order: &OrderReference,
) -> Result<String, ScenarioError> {
    let hash = hand_in_bond_claim(ctx, actor, order, ClaimInvoice::Plain).await?;
    super::expect_payout_settled(ctx, actor, order, &hash).await?;
    wait_bond_claim_phase(ctx, actor, order, CLAIM_COMPLETED).await?;
    Ok(hash)
}

/// `actor` claims their share with a hold invoice from their node and
/// withdraws it once the daemon's payment is held there: the payment fails,
/// and the daemon, its retries spent, asks for a new invoice. The withdrawn
/// one is recorded superseded before the node cancels it, must never
/// settle, and the app must read the claim pending again. Returns its hash.
pub async fn fail_bond_claim(
    ctx: &Context,
    actor: Actor,
    order: &OrderReference,
) -> Result<String, ScenarioError> {
    let hash = hand_in_bond_claim(ctx, actor, order, ClaimInvoice::Hold).await?;
    let held = super::wait_hold_invoice(
        ctx,
        actor,
        order,
        &hash,
        HoldInvoiceState::Accepted,
        ctx.state_timeout,
    )
    .await;
    // Mortsom never settles a hold claim invoice: whether or not the
    // daemon's payment arrived, it is withdrawn.
    withdraw_claim_invoice(ctx, actor, order, &hash, "with the daemon's payment held").await?;
    held?;
    super::expect_payout_never_settled(ctx, actor, order, &hash).await?;
    wait_bond_claim_phase(ctx, actor, order, CLAIM_PENDING).await?;
    Ok(hash)
}

/// Withdraws the claim invoice `hash` on `actor`'s node: recorded
/// superseded first, so recovery holds it to never settling, then
/// cancelled, which fails any payment the daemon holds there.
async fn withdraw_claim_invoice(
    ctx: &Context,
    actor: Actor,
    order: &OrderReference,
    hash: &str,
    why: &str,
) -> Result<(), ScenarioError> {
    let node = claim_node(ctx, actor)?;
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
            "wallet: claim invoice {} for order {} withdrawn {why}",
            short(hash),
            order.id
        ),
        Some(hash.to_owned()),
    );
    Ok(())
}

/// The node Mortsom holds for `actor`, which a claim is paid to.
fn claim_node(
    ctx: &Context,
    actor: Actor,
) -> Result<&std::sync::Arc<dyn NodeWallet>, ScenarioError> {
    ctx.node_wallet(actor).ok_or_else(|| {
        ScenarioError::environment(format!(
            "the claim is paid to a node Mortsom holds, and none is configured for {actor}"
        ))
    })
}

/// Hands in an invoice of `kind` for `actor`'s pending claim on `order`,
/// for exactly the share the declared policy leaves the winner, once the
/// app offers that share; waits until the app reads the claim on its way.
/// Returns the invoice's hash.
async fn hand_in_bond_claim(
    ctx: &Context,
    actor: Actor,
    order: &OrderReference,
    kind: ClaimInvoice,
) -> Result<String, ScenarioError> {
    let policy = bond_policy(ctx)?;
    let expected = policy.counterparty_share_sats(policy.bond_sats(ctx.amount_sats));
    let node = claim_node(ctx, actor)?;
    let claim = wait_bond_claim(ctx, actor, order).await?;
    if claim.status != "pending" {
        return Err(ScenarioError::product(format!(
            "the claim on order {} reads `{}` before any invoice was sent",
            order.id, claim.status
        )));
    }
    if claim.amount_sats != expected {
        return Err(ScenarioError::product(format!(
            "the app offers {} sats on order {}; the declared policy leaves the winner {expected}",
            claim.amount_sats, order.id
        )));
    }
    let memo = format!("mortsom bond claim {}", order.id);
    let invoice = match kind {
        ClaimInvoice::Plain => node.make_invoice(expected, &memo).await,
        ClaimInvoice::Hold => node.make_hold_invoice(expected, &memo).await,
    }
    .map_err(ScenarioError::environment)?;
    ctx.protect(&invoice);
    let hash = payment_hash_of(&invoice).map_err(|e| ScenarioError::wallet(e.to_string()))?;
    ctx.journal
        .record(RecoveryEvent::PaymentExpected {
            order_id: order.id.clone(),
            wallet: actor.as_str().into(),
            hash: hash.clone(),
            incoming: true,
            amount_sats: expected,
            // The share of a slashed bond: it settles on an order that
            // did not complete, unlike the trade's own payout.
            bond: true,
        })
        .map_err(ScenarioError::environment)?;
    let session = ctx.actor(actor)?;
    let submitted = ctx
        .adapter
        .submit_bond_claim_invoice(&session.device(ctx.ui_timeout), order, &invoice)
        .await;
    let phase = match submitted {
        Ok(phase) => phase,
        Err(error) => {
            // Never handed over, or not as far as anyone can tell: the
            // invoice is withdrawn rather than left open for recovery to
            // wait on.
            withdraw_claim_invoice(ctx, actor, order, &hash, "after the app did not take it")
                .await?;
            return Err(error.into());
        }
    };
    let step = ctx
        .evidence
        .checkpoint(actor.as_str(), "bond-claim", session.ui.as_ref())
        .await;
    let what = match kind {
        ClaimInvoice::Plain => "an invoice",
        ClaimInvoice::Hold => "a hold invoice",
    };
    ctx.timeline.push(
        TimelineKind::Step,
        Some(actor.as_str()),
        format!(
            "claimed {expected} sats of the slashed bond on order {} with {what} (hash {}; the app reads {phase})",
            order.id,
            short(&hash)
        ),
        Some(step),
    );
    if !CLAIM_IN_PROGRESS.contains(&phase.as_str()) {
        wait_bond_claim_phase(ctx, actor, order, CLAIM_IN_PROGRESS).await?;
    }
    Ok(hash)
}

/// `actor`'s app shows their own bond slashed on `order` for `cause`
/// (`dispute` or `timeout`), within the state timeout.
pub async fn expect_bond_slash_notice(
    ctx: &Context,
    actor: Actor,
    order: &OrderReference,
    cause: &str,
) -> Result<(), ScenarioError> {
    let session = ctx.actor(actor)?;
    let device = session.device(ctx.ui_timeout);
    let found = wait_until(ctx.state_timeout, STATE_POLL, || async {
        match ctx.adapter.bond_slash_notice(&device, order).await {
            Ok(Some(shown)) => Some(shown),
            _ => None,
        }
    })
    .await;
    match found {
        Some(shown) if shown == cause => {
            ctx.timeline.push(
                TimelineKind::UiState,
                Some(actor.as_str()),
                format!(
                    "{actor}'s app shows their bond on order {} slashed ({cause})",
                    order.id
                ),
                None,
            );
            Ok(())
        }
        Some(shown) => Err(ScenarioError::product(format!(
            "{actor}'s app shows the bond on order {} slashed for `{shown}`, not `{cause}`",
            order.id
        ))),
        None => Err(ScenarioError::product(format!(
            "{actor}'s app shows no slash notice on order {} within {:?}",
            order.id, ctx.state_timeout
        ))),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::context::{BondApplyTo, BondPolicy};
    use crate::test_support::{StateAdapter, context};
    use mortsom_common::FailureCategory;
    use mortsom_observers::wallet::{WalletObserver, WalletReader};
    use std::sync::{Arc, Mutex};

    fn policy() -> BondPolicy {
        BondPolicy {
            apply_to: BondApplyTo::Take,
            amount_bps: 100,
            base_amount_sats: 1000,
            slash_on_waiting_timeout: false,
            node_share_bps: 5000,
            payout_claim_window_days: 1,
        }
    }

    /// A cleared regtest context: value may move, within caps of 10 sats.
    fn ctx() -> Context {
        let mut ctx = context(Arc::new(StateAdapter(Mutex::new(OrderState::Taken))));
        ctx.mainnet = Some(crate::context::MainnetClearance {
            lane: mortsom_common::Lane::Local,
        });
        ctx
    }

    /// The bond the test contexts hold in flight, within the caps.
    const BOND_SATS: u64 = 5;

    /// Without `stack up --bond` no bond is ever asked for: the scenario
    /// is an environment error before it touches the app or the budget.
    #[tokio::test]
    async fn a_stack_without_a_bond_fails_the_scenario_as_environment_before_any_payment() {
        let ctx = ctx();
        assert!(ctx.limits.bond.is_none());

        let err = pay_bond(&ctx, Actor::Bob, &OrderReference::new("o1"))
            .await
            .unwrap_err();

        assert_eq!(err.category, FailureCategory::EnvironmentMisconfiguration);
        assert!(err.message.contains("stack up --bond"), "{err}");
        assert!(!ctx.budget.lock().unwrap().has_open_reservations());
    }

    /// The bond is paid from a node Mortsom holds; an actor without one
    /// cannot pay it, and the app is never asked to.
    #[tokio::test]
    async fn an_actor_without_a_node_wallet_cannot_pay_a_bond() {
        let mut ctx = ctx();
        ctx.limits.bond = Some(policy());
        ctx.node_wallets.clear();

        let err = pay_bond(&ctx, Actor::Bob, &OrderReference::new("o1"))
            .await
            .unwrap_err();

        assert_eq!(err.category, FailureCategory::EnvironmentMisconfiguration);
        assert!(err.message.contains("node"), "{err}");
        assert!(!ctx.budget.lock().unwrap().has_open_reservations());
    }

    struct Reader(Mutex<PaymentState>);
    #[async_trait::async_trait]
    impl WalletReader for Reader {
        async fn lookup(&self, _: &str) -> Result<Option<(u64, u64, PaymentState)>, String> {
            Ok(Some((BOND_SATS, 0, *self.0.lock().unwrap())))
        }
    }

    /// A context with a bond in flight from Bob's node for `o1`, and the
    /// wallet reader that decides its fate.
    fn with_bond_in_flight(fate: PaymentState) -> Context {
        let mut ctx = ctx();
        let reader = Arc::new(Reader(Mutex::new(fate)));
        ctx.wallets = Arc::new(
            WalletObserver::new(vec![("bob".into(), reader as Arc<dyn WalletReader>)])
                .with_poll(Duration::from_millis(1)),
        );
        ctx.settle_timeout = Duration::from_millis(50);
        let reservation = ctx.budget.lock().unwrap().reserve("o1", BOND_SATS).unwrap();
        ctx.budget
            .lock()
            .unwrap()
            .mark_in_flight(&reservation, Some("bondhash".into()))
            .unwrap();
        ctx.record_pending_hold(
            &bond_key(Actor::Bob, &OrderReference::new("o1")),
            PendingHold {
                actor: Actor::Bob,
                wallet_label: "bob".into(),
                payment_hash: "bondhash".into(),
                reservation,
            },
        )
        .unwrap();
        ctx
    }

    #[tokio::test]
    async fn a_bond_that_comes_back_frees_its_reservation() {
        let ctx = with_bond_in_flight(PaymentState::Failed);

        expect_bond_released(&ctx, Actor::Bob, &OrderReference::new("o1"))
            .await
            .unwrap();

        assert!(!ctx.budget.lock().unwrap().has_open_reservations());
        assert!(ctx.pending_holds.lock().unwrap().is_empty());
        assert!(
            ctx.timeline
                .entries()
                .iter()
                .any(|entry| entry.summary.contains("returned"))
        );
    }

    /// The daemon settled the bond on an honest exit: the sats left Bob's
    /// wallet, which the accounting records before the product failure is
    /// reported.
    #[tokio::test]
    async fn a_bond_that_settles_on_an_honest_exit_is_a_product_failure_and_is_accounted() {
        let ctx = with_bond_in_flight(PaymentState::Settled);

        let err = expect_bond_released(&ctx, Actor::Bob, &OrderReference::new("o1"))
            .await
            .unwrap_err();

        assert_eq!(err.category, FailureCategory::Product);
        assert!(err.message.contains("slashed"), "{err}");
        assert!(!ctx.budget.lock().unwrap().has_open_reservations());
        assert_eq!(ctx.budget.lock().unwrap().totals().settled_sats, BOND_SATS);
    }

    /// The bond's slot is the order's own escrow slot plus a suffix: the
    /// seller of a buy holds both on one order, and resolving one must
    /// not touch the other.
    #[tokio::test]
    async fn the_bond_and_the_escrow_of_one_order_are_resolved_apart() {
        let ctx = with_bond_in_flight(PaymentState::Failed);
        let escrow = ctx.budget.lock().unwrap().reserve("o1", 4).unwrap();
        ctx.record_pending_hold(
            "o1",
            PendingHold {
                actor: Actor::Alice,
                wallet_label: "alice".into(),
                payment_hash: "escrowhash".into(),
                reservation: escrow,
            },
        )
        .unwrap();

        expect_bond_released(&ctx, Actor::Bob, &OrderReference::new("o1"))
            .await
            .unwrap();

        let pending = ctx.pending_holds.lock().unwrap();
        assert!(pending.contains_key("o1"), "the escrow is still pending");
        assert!(!pending.contains_key("o1#bond:bob"));
    }

    /// A second payment for an occupied slot is refused, never written over
    /// the first, which would then stay in flight with nothing to resolve it.
    #[tokio::test]
    async fn a_slot_already_holding_a_bond_is_never_replaced() {
        let ctx = with_bond_in_flight(PaymentState::Failed);
        let again = ctx.budget.lock().unwrap().reserve("o1", 1).unwrap();

        let err = ctx
            .record_pending_hold(
                &bond_key(Actor::Bob, &OrderReference::new("o1")),
                PendingHold {
                    actor: Actor::Bob,
                    wallet_label: "bob".into(),
                    payment_hash: "otherhash".into(),
                    reservation: again,
                },
            )
            .unwrap_err();

        assert_eq!(err.category, FailureCategory::EnvironmentMisconfiguration);
        let pending = ctx.pending_holds.lock().unwrap();
        assert_eq!(pending["o1#bond:bob"].payment_hash, "bondhash");
    }

    /// With both sides bonded, one order holds the maker's bond, the
    /// taker's and the escrow, each with its fee ceiling: a cap that holds
    /// one bond and the escrow is not enough.
    #[test]
    fn the_room_check_counts_every_bond_the_order_holds() {
        let mut ctx = ctx();
        ctx.amount_sats = 1000;
        *ctx.budget.lock().unwrap() =
            crate::budget::Budget::new(crate::budget::SpendCaps::new(2500, 100_000).unwrap());
        ctx.limits.bond = Some(policy());
        require_room_for_bond_and_escrow(&ctx).unwrap();

        ctx.limits.bond = Some(BondPolicy {
            apply_to: BondApplyTo::Both,
            ..policy()
        });
        let err = require_room_for_bond_and_escrow(&ctx).unwrap_err();

        assert_eq!(err.category, FailureCategory::EnvironmentMisconfiguration);
        assert!(err.message.contains("3000 sats"), "{err}");
    }

    /// A seller's bond and escrow share one order's cap: a cap that cannot
    /// hold both is refused before anything is paid.
    #[test]
    fn a_cap_too_small_for_the_bond_and_the_escrow_is_refused_up_front() {
        let mut ctx = ctx();
        ctx.limits.bond = Some(policy());

        let err = require_room_for_bond_and_escrow(&ctx).unwrap_err();

        assert_eq!(err.category, FailureCategory::EnvironmentMisconfiguration);
        assert!(err.message.contains("max_order_sats"), "{err}");
    }

    #[tokio::test]
    async fn without_a_bond_in_flight_there_is_nothing_to_resolve() {
        let ctx = ctx();
        expect_bond_released(&ctx, Actor::Bob, &OrderReference::new("o1"))
            .await
            .unwrap();
        let err = expect_bond_slashed(&ctx, Actor::Bob, &OrderReference::new("o1"))
            .await
            .unwrap_err();
        assert_eq!(err.category, FailureCategory::EnvironmentMisconfiguration);
    }

    /// A slashed bond settles on the payer's node: the spend is recorded
    /// and the slot freed.
    #[tokio::test]
    async fn a_slashed_bond_settles_and_is_accounted() {
        let ctx = with_bond_in_flight(PaymentState::Settled);

        expect_bond_slashed(&ctx, Actor::Bob, &OrderReference::new("o1"))
            .await
            .unwrap();

        assert!(!ctx.budget.lock().unwrap().has_open_reservations());
        assert_eq!(ctx.budget.lock().unwrap().totals().settled_sats, BOND_SATS);
        assert!(ctx.pending_holds.lock().unwrap().is_empty());
    }

    /// A bond that came back was not slashed: expecting a slash is a
    /// product failure.
    #[tokio::test]
    async fn a_returned_bond_is_not_a_slash() {
        let ctx = with_bond_in_flight(PaymentState::Failed);

        let err = expect_bond_slashed(&ctx, Actor::Bob, &OrderReference::new("o1"))
            .await
            .unwrap_err();

        assert_eq!(err.category, FailureCategory::Wallet);
    }
}
