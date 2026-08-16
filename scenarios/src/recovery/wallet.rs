//! Wallet-side hiccups (Phase 5.3) that Mortsom can provoke without a
//! controllable wallet: cancelling instead of paying through NWC, and the
//! wallet being unreachable when the seller first tries to pay. Rejected,
//! expired and failed-payout invoices need a controllable wallet provider
//! (spec §6) and are not modelled against real NWC wallets.

use mortsom_android_driver::avd::Actor;
use mortsom_app_contract::{Capability, OrderState};
use mortsom_scenario_kit::cancel::cancel_payment;
use mortsom_scenario_kit::context::{Context, ScenarioError};
use mortsom_scenario_kit::recovery::payment_fails_while_offline;

use super::{NO_VALUE_TIMEOUT_SECS, TRADE_TIMEOUT_SECS};
use crate::flow;

/// Capabilities for the cancel-instead-of-pay path.
pub const NWC_CANCEL_REQUIRES: &[Capability] = &[
    Capability::Onboarding,
    Capability::CustomNodeSelection,
    Capability::LocalRelayConfiguration,
    Capability::Nwc,
    Capability::CreateOrder,
    Capability::TakeOrder,
    Capability::BuyerInvoice,
    Capability::HoldInvoicePayment,
    Capability::Cancel,
];

/// The seller opens the pay screen and cancels instead of paying: no sats
/// move and the order is cancelled for both.
pub async fn nwc_cancel_payment(ctx: &Context) -> Result<(), ScenarioError> {
    let taken = flow::create_and_take(ctx).await?;
    cancel_payment(ctx, Actor::Alice, &taken.order).await?;
    flow::expect_cancelled(ctx, &taken.order).await
}

/// The seller's wallet is unreachable (device offline) when the order is
/// ready to fund; once connectivity returns the payment goes through and
/// the trade completes.
pub async fn wallet_unavailable_then_pay(ctx: &Context) -> Result<(), ScenarioError> {
    let taken = flow::create_and_take(ctx).await?;
    // The point of the scenario: funding is attempted while the wallet is
    // out of reach, not merely after an outage that already ended.
    payment_fails_while_offline(ctx, Actor::Alice, &taken.order, OrderState::Taken).await?;
    flow::fund(ctx, &taken.order).await?;
    flow::fiat_sent(ctx, &taken.order).await?;
    flow::release_and_settle(ctx, &taken).await
}

crate::register_scenario! {
    name: "nwc_cancel_payment",
    body: nwc_cancel_payment,
    tags: &["recovery", "wallet"],
    requires: NWC_CANCEL_REQUIRES,
    timeout_secs: NO_VALUE_TIMEOUT_SECS,
    moves_value: false,
}
crate::register_scenario! {
    name: "wallet_unavailable_then_pay",
    body: wallet_unavailable_then_pay,
    tags: &["recovery", "wallet"],
    requires: flow::TRADE_REQUIRES,
    timeout_secs: TRADE_TIMEOUT_SECS,
    moves_value: true,
}

/// Keeps registrations linked.
pub fn link() {}
