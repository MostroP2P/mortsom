//! The bonds a bonded daemon slashes, and the winner's claim of the share
//! it does not keep. A waiting-state timeout slashes whoever owed the step
//! (`slash_on_waiting_timeout`): the buyer who never invoices, the seller
//! who never funds, maker or taker alike. A taker's failure puts the order
//! back on the book; a maker's closes it, and the other side's bond comes
//! back. A solver slashes by a `BondResolution` on its ruling, whatever
//! the trade's outcome. The winner then reads the claim in their app,
//! hands in an invoice from their node, and is paid; one the daemon cannot
//! pay is asked for again.

use super::*;

/// The environment error a timeout scenario ends with on a stack whose
/// policy never slashes on a waiting timeout.
fn require_timeout_slash(ctx: &Context) -> Result<(), ScenarioError> {
    if actions::bond_policy(ctx)?.slash_on_waiting_timeout {
        return Ok(());
    }
    Err(ScenarioError::environment(
        "the stack's bond policy does not slash on a waiting timeout",
    ))
}

/// Waits for the daemon's waiting-state clock to end the step handed out
/// after `since`: the book shows `outcome`, a fresh `pending` for a taker
/// who failed, `canceled` for a maker who did.
async fn wait_for_the_clock(
    ctx: &Context,
    order: &OrderReference,
    since: u64,
    outcome: OrderState,
) -> Result<(), ScenarioError> {
    actions::wait_relay_state_since(ctx, order, &[outcome], since, ctx.expiration_timeout)
        .await
        .map(|_| ())
}

/// Alice's bonded sell order, taken by Bob whose bond locked, and Bob
/// never invoicing: the buyer's deadline slashes his bond and the order is
/// back on the book, his app no longer holding it. Alice's claim is next.
async fn buyer_taker_slashed(ctx: &Context) -> Result<OrderReference, ScenarioError> {
    require_timeout_slash(ctx)?;
    let order = sell_taken_with_bonds(ctx).await?;
    wait_for_the_clock(ctx, &order, actions::relay_cursor(ctx), OrderState::Pending).await?;
    actions::expect_bond_slashed(ctx, Actor::Bob, &order).await?;
    actions::expect_closed(ctx, Actor::Bob, &order, ctx.state_timeout).await?;
    actions::wait_app_state(ctx, Actor::Alice, &order, OrderState::Pending).await?;
    Ok(order)
}

/// Alice creates a sell order. Bob takes it, pays his bond, and never
/// hands in his invoice. When the buyer's deadline elapses the daemon
/// slashes Bob's bond and puts the order back on the book. Bob's payment
/// settles on his node and his app no longer lists the order; Alice is
/// offered the share the node does not keep, claims it with an invoice
/// from her node, and is paid. She then cancels the order, and her own
/// bond comes back.
pub async fn buyer_taker_bond_slashed_on_timeout(ctx: &Context) -> Result<(), ScenarioError> {
    let order = buyer_taker_slashed(ctx).await?;
    actions::claim_bond_payout(ctx, Actor::Alice, &order).await?;
    cancel_and_release_maker(ctx, Actor::Alice, &order).await
}

/// As `buyer_taker_bond_slashed_on_timeout` up to Alice's claim; she
/// claims with a hold invoice and withdraws it while the daemon's payment
/// is held on her node. The daemon's payment fails, and it asks her for a
/// new invoice: her app reads the claim pending again. The replacement is
/// paid and the claim completes; the withdrawn invoice never settles.
pub async fn bond_claim_fails_then_replaced(ctx: &Context) -> Result<(), ScenarioError> {
    let order = buyer_taker_slashed(ctx).await?;
    actions::fail_bond_claim(ctx, Actor::Alice, &order).await?;
    actions::claim_bond_payout(ctx, Actor::Alice, &order).await?;
    cancel_and_release_maker(ctx, Actor::Alice, &order).await
}

/// Alice creates a sell order. Bob takes it, pays his bond and hands in
/// his invoice; Alice never funds the escrow. When the seller's deadline
/// elapses the daemon slashes Alice's maker bond and closes the order,
/// returning Bob's bond. Alice's app no longer holds the order; Bob claims
/// the share and is paid.
pub async fn seller_maker_bond_slashed_on_timeout(ctx: &Context) -> Result<(), ScenarioError> {
    require_timeout_slash(ctx)?;
    require_maker_bond(ctx)?;
    let order = sell_taken_with_bonds(ctx).await?;
    actions::wait_app_label(ctx, Actor::Bob, &order, WAITING_INVOICE).await?;
    actions::provide_buyer_invoice(ctx, Actor::Bob, &order).await?;
    actions::wait_app_label(ctx, Actor::Alice, &order, WAITING_PAYMENT).await?;
    wait_for_the_clock(
        ctx,
        &order,
        actions::relay_cursor(ctx),
        OrderState::Cancelled,
    )
    .await?;
    actions::expect_bond_slashed(ctx, Actor::Alice, &order).await?;
    actions::expect_bond_released(ctx, Actor::Bob, &order).await?;
    actions::expect_closed(ctx, Actor::Alice, &order, ctx.state_timeout).await?;
    actions::claim_bond_payout(ctx, Actor::Bob, &order).await?;
    actions::mark_resolved(ctx, &order);
    Ok(())
}

/// Bob creates a buy order. Alice takes it as seller, pays her bond, and
/// never funds the escrow. When the seller's deadline elapses the daemon
/// slashes Alice's bond and puts the order back on the book. Her app no
/// longer holds it; Bob's shows it pending, and he claims the share and is
/// paid. He then cancels the order, and his own bond comes back.
pub async fn seller_taker_bond_slashed_on_timeout(ctx: &Context) -> Result<(), ScenarioError> {
    require_timeout_slash(ctx)?;
    let order = buy_taken_with_bonds(ctx).await?;
    wait_for_the_clock(ctx, &order, actions::relay_cursor(ctx), OrderState::Pending).await?;
    actions::expect_bond_slashed(ctx, Actor::Alice, &order).await?;
    actions::expect_closed(ctx, Actor::Alice, &order, ctx.state_timeout).await?;
    actions::wait_app_state(ctx, Actor::Bob, &order, OrderState::Pending).await?;
    actions::claim_bond_payout(ctx, Actor::Bob, &order).await?;
    cancel_and_release_maker(ctx, Actor::Bob, &order).await
}

/// Bob creates a buy order. Alice takes it as seller, pays her bond and
/// the escrow; Bob never hands in his invoice. When the buyer's deadline
/// elapses the daemon slashes Bob's maker bond and closes the order: the
/// escrow and Alice's bond come back to her node. Bob's app no longer holds
/// the order; Alice claims the share and is paid.
pub async fn buyer_maker_bond_slashed_on_timeout(ctx: &Context) -> Result<(), ScenarioError> {
    require_timeout_slash(ctx)?;
    require_maker_bond(ctx)?;
    let order = buy_taken_with_bonds(ctx).await?;
    actions::pay_hold_invoice(ctx, Actor::Alice, &order).await?;
    actions::wait_app_label(ctx, Actor::Bob, &order, WAITING_INVOICE).await?;
    wait_for_the_clock(
        ctx,
        &order,
        actions::relay_cursor(ctx),
        OrderState::Cancelled,
    )
    .await?;
    actions::expect_bond_slashed(ctx, Actor::Bob, &order).await?;
    actions::expect_hold_refunded(ctx, &order).await?;
    actions::expect_bond_released(ctx, Actor::Alice, &order).await?;
    actions::expect_closed(ctx, Actor::Bob, &order, ctx.state_timeout).await?;
    actions::claim_bond_payout(ctx, Actor::Alice, &order).await?;
    actions::mark_resolved(ctx, &order);
    Ok(())
}

/// A funded, bonded trade in dispute: the solver cancels it for Alice and
/// directs the daemon to slash the buyer's bond. The escrow comes back to
/// Alice, Bob's bond settles on his node and his trade shows the slash,
/// Alice claims the share and is paid, and her own bond comes back.
pub async fn taker_bond_slashed_by_solver(ctx: &Context) -> Result<(), ScenarioError> {
    let (taken, dispute) = funded_trade_in_dispute(ctx, Actor::Alice).await?;
    let since = actions::relay_cursor(ctx);
    let slash = BondSlash {
        seller: false,
        buyer: true,
    };
    actions::solver_rules_slashing(ctx, &taken.order, false, slash).await?;
    actions::wait_dispute_status(
        ctx,
        &dispute.id,
        actions::dispute_status::SELLER_REFUNDED,
        since,
    )
    .await?;
    actions::wait_both(ctx, &taken.order, OrderState::Cancelled).await?;
    actions::wait_relay_state_since(
        ctx,
        &taken.order,
        &[OrderState::Cancelled],
        since,
        ctx.state_timeout,
    )
    .await?;
    actions::expect_hold_refunded(ctx, &taken.order).await?;
    actions::expect_bond_slashed(ctx, Actor::Bob, &taken.order).await?;
    actions::expect_bond_slash_notice(ctx, Actor::Bob, &taken.order, "dispute").await?;
    actions::claim_bond_payout(ctx, Actor::Alice, &taken.order).await?;
    actions::expect_bond_released(ctx, Actor::Alice, &taken.order).await?;
    actions::mark_resolved(ctx, &taken.order);
    Ok(())
}

/// A funded, bonded trade in dispute, opened by Bob after Alice did not
/// release: the solver settles it for Bob and directs the daemon to slash
/// the seller's bond, Alice's as maker. The escrow settles and Bob's payout
/// lands; Alice's bond settles on her node and her trade shows the slash;
/// Bob's bond comes back, and he claims the share and is paid.
pub async fn maker_bond_slashed_by_solver(ctx: &Context) -> Result<(), ScenarioError> {
    require_maker_bond(ctx)?;
    let (taken, dispute) = funded_trade_in_dispute(ctx, Actor::Bob).await?;
    let hash = taken
        .payout_hash
        .as_deref()
        .ok_or_else(|| ScenarioError::wallet("buyer payout hash is required before the ruling"))?;
    let since = actions::relay_cursor(ctx);
    let slash = BondSlash {
        seller: true,
        buyer: false,
    };
    actions::solver_rules_slashing(ctx, &taken.order, true, slash).await?;
    actions::wait_dispute_status(ctx, &dispute.id, actions::dispute_status::SETTLED, since).await?;
    actions::wait_both(ctx, &taken.order, OrderState::Completed).await?;
    actions::wait_relay_state_since(
        ctx,
        &taken.order,
        &[OrderState::Completed],
        since,
        ctx.state_timeout,
    )
    .await?;
    actions::settle_hold_payment(ctx, &taken.order).await?;
    actions::expect_payout_settled(ctx, Actor::Bob, &taken.order, hash).await?;
    actions::expect_bond_slashed(ctx, Actor::Alice, &taken.order).await?;
    actions::expect_bond_released(ctx, Actor::Bob, &taken.order).await?;
    actions::expect_bond_slash_notice(ctx, Actor::Alice, &taken.order, "dispute").await?;
    actions::claim_bond_payout(ctx, Actor::Bob, &taken.order).await?;
    actions::mark_resolved(ctx, &taken.order);
    Ok(())
}

crate::register_scenario! {
    name: "buyer_taker_bond_slashed_on_timeout",
    body: buyer_taker_bond_slashed_on_timeout,
    tags: &["bond"],
    requires: BOND_TAKE_REQUIRES,
    timeout_secs: SLASH_TIMEOUT_SECS,
    moves_value: true,
}
crate::register_scenario! {
    name: "bond_claim_fails_then_replaced",
    body: bond_claim_fails_then_replaced,
    tags: &["bond"],
    requires: BOND_TAKE_REQUIRES,
    timeout_secs: SLASH_TIMEOUT_SECS,
    moves_value: true,
}
crate::register_scenario! {
    name: "seller_maker_bond_slashed_on_timeout",
    body: seller_maker_bond_slashed_on_timeout,
    tags: &["bond"],
    requires: BOND_ESCROW_REQUIRES,
    timeout_secs: SLASH_TIMEOUT_SECS,
    moves_value: true,
}
crate::register_scenario! {
    name: "seller_taker_bond_slashed_on_timeout",
    body: seller_taker_bond_slashed_on_timeout,
    tags: &["bond"],
    requires: BOND_ESCROW_REQUIRES,
    timeout_secs: SLASH_TIMEOUT_SECS,
    moves_value: true,
}
crate::register_scenario! {
    name: "buyer_maker_bond_slashed_on_timeout",
    body: buyer_maker_bond_slashed_on_timeout,
    tags: &["bond"],
    requires: BOND_ESCROW_REQUIRES,
    timeout_secs: SLASH_TIMEOUT_SECS,
    moves_value: true,
}
crate::register_scenario! {
    name: "taker_bond_slashed_by_solver",
    body: taker_bond_slashed_by_solver,
    tags: &["bond"],
    requires: BOND_DISPUTE_REQUIRES,
    timeout_secs: TRADE_TIMEOUT_SECS,
    moves_value: true,
    needs_app_wallet: false,
    needs_solver: true,
}
crate::register_scenario! {
    name: "maker_bond_slashed_by_solver",
    body: maker_bond_slashed_by_solver,
    tags: &["bond"],
    requires: BOND_DISPUTE_REQUIRES,
    timeout_secs: TRADE_TIMEOUT_SECS,
    moves_value: true,
    needs_app_wallet: false,
    needs_solver: true,
}

#[cfg(test)]
#[path = "tests_slash.rs"]
mod tests;
