//! Mortsom adapter for [mostro-cli](https://github.com/MostroP2P/mostro-cli)
//! (profile `cli`): the business capabilities of [`AppAdapter`] over the
//! CLI's JSON machine interface, one subprocess per action.
//!
//! The CLI holds no wallet, so every payment takes the external-wallet
//! path: Mortsom pays from the node it holds and hands the CLI the invoices
//! it issued there. What the CLI can do is read from its own offline
//! discovery (`mostro-cli schema`): a capability whose commands the built
//! CLI lacks is not claimed, so its scenarios are skipped with a reason.

mod access;
mod bond;
mod identity;
mod memo;
mod messages;
mod orders;
mod payments;
mod trade;

pub use messages::MessageWindow;

use std::collections::BTreeSet;
use std::time::Duration;

use async_trait::async_trait;
use mortsom_app_contract::{
    AdapterError, AppAdapter, AppOrderState, AppProfile, BondClaim, CancelOutcome, Capability,
    CapabilitySet, DeviceContext, Identity, InvoiceSubmission, OrderDraft, OrderReference,
    PaymentHandoff, PaymentPath, TestEnvironment, VisibleOrder, WalletConnection,
};
use mortsom_common::Secret;

/// The profile this adapter drives.
pub const PROFILE: &str = "cli";

/// How often a wait for a node request reads the actor's messages.
const POLL: Duration = Duration::from_secs(2);

/// Commands every capability needs, by discovery name.
const CAPABILITY_COMMANDS: &[(Capability, &[&str])] = &[
    (Capability::Onboarding, &["identity.init", "identity.show"]),
    (Capability::CustomNodeSelection, &["order.list"]),
    (Capability::LocalRelayConfiguration, &["order.list"]),
    (
        Capability::CreateOrder,
        &["order.new", "order.info", "order.mine"],
    ),
    (Capability::MarketPriceOrder, &["order.new"]),
    (Capability::RangeOrder, &["order.new", "order.sync"]),
    (
        Capability::TakeOrder,
        &["order.take-sell", "order.take-buy"],
    ),
    (Capability::ExternalWallet, &["message.list"]),
    (Capability::BuyerInvoice, &["order.add-invoice"]),
    (Capability::FiatSent, &["order.fiat-sent"]),
    (Capability::Release, &["order.release"]),
    (Capability::CompletedObservation, &["order.info"]),
    (Capability::Cancel, &["order.cancel"]),
    (Capability::Dispute, &["dispute.open"]),
    (Capability::Rating, &["order.rate"]),
    (Capability::Bond, &["bond.add-invoice", "message.list"]),
];

/// What the adapter knows about the run and the CLI under test.
#[derive(Debug, Clone, Default)]
pub struct CliAdapterConfig {
    /// Seconds every created order asks the daemon to expire it after
    /// (`mobile.order_expiry_secs`); `None` leaves the daemon's default.
    pub order_expiry_secs: Option<u64>,
    /// Commands the built CLI offers (`mostro-cli schema`), by dotted name.
    pub commands: BTreeSet<String>,
    /// Whether `order new` accepts a lifetime in seconds (`--expires-in`).
    pub order_expiry_in_seconds: bool,
    /// How far back the actor's messages are read (`message list --since`):
    /// a run's window by default; a recovery's reaches back to the run's
    /// start ([`MessageWindow::SinceRunStart`]).
    pub message_window: MessageWindow,
}

/// The `cli` profile.
#[derive(Debug, Default)]
pub struct CliAdapter {
    config: CliAdapterConfig,
    memo: memo::Memo,
    poll: Duration,
}

impl CliAdapter {
    /// An adapter for a CLI offering `config.commands`.
    pub fn new(config: CliAdapterConfig) -> Self {
        Self {
            config,
            memo: memo::Memo::default(),
            poll: POLL,
        }
    }

    /// Reads node requests every `poll` (tests).
    pub fn with_poll(mut self, poll: Duration) -> Self {
        self.poll = poll;
        self
    }

    fn offers(&self, command: &str) -> bool {
        self.config.commands.contains(command)
    }

    /// The lifetime `order new` asks for, when the CLI can ask for one.
    fn expiry(&self) -> Option<u64> {
        self.config
            .order_expiry_in_seconds
            .then_some(self.config.order_expiry_secs)
            .flatten()
    }
}

#[async_trait]
impl AppAdapter for CliAdapter {
    fn profile(&self) -> AppProfile {
        AppProfile {
            name: PROFILE.into(),
        }
    }

    /// Every capability whose commands the CLI offers. There is no wallet
    /// (`nwc`, `hold-invoice-payment`), no screen to offer actions on, and
    /// no notifications, deep links or build marker.
    fn capabilities(&self) -> CapabilitySet {
        let mut supported: Vec<Capability> = CAPABILITY_COMMANDS
            .iter()
            .filter(|(_, commands)| commands.iter().all(|c| self.offers(c)))
            .map(|(capability, _)| *capability)
            .collect();
        if self.config.order_expiry_in_seconds && self.offers("order.new") {
            supported.push(Capability::OrderExpiry);
        }
        CapabilitySet::new(supported)
    }

    async fn onboard(
        &self,
        device: &DeviceContext<'_>,
        _env: &TestEnvironment,
    ) -> Result<Identity, AdapterError> {
        identity::onboard(device).await
    }

    async fn displayed_identity(
        &self,
        device: &DeviceContext<'_>,
    ) -> Result<Identity, AdapterError> {
        identity::displayed(device).await
    }

    async fn existing_identity(
        &self,
        device: &DeviceContext<'_>,
    ) -> Result<Identity, AdapterError> {
        identity::displayed(device).await
    }

    async fn configure_node_and_relay(
        &self,
        device: &DeviceContext<'_>,
        env: &TestEnvironment,
    ) -> Result<(), AdapterError> {
        identity::check_node(device, env).await
    }

    async fn configure_nwc(
        &self,
        _device: &DeviceContext<'_>,
        _label: &str,
        _uri: &Secret,
    ) -> Result<WalletConnection, AdapterError> {
        Err(AdapterError::contract(
            "mostro-cli holds no wallet; only the external-wallet path exists",
        ))
    }

    async fn owned_orders(&self, device: &DeviceContext<'_>) -> Result<Vec<String>, AdapterError> {
        orders::owned(
            device,
            self.offers("order.sync"),
            self.config.message_window,
        )
        .await
    }

    async fn create_order(
        &self,
        device: &DeviceContext<'_>,
        draft: &OrderDraft,
    ) -> Result<VisibleOrder, AdapterError> {
        orders::create(device, &self.memo, draft, self.expiry()).await
    }

    async fn find_and_take_order(
        &self,
        device: &DeviceContext<'_>,
        order: &OrderReference,
    ) -> Result<(), AdapterError> {
        orders::take(device, &self.memo, order, None).await
    }

    async fn find_and_take_order_at(
        &self,
        device: &DeviceContext<'_>,
        order: &OrderReference,
        fiat_amount: u64,
    ) -> Result<(), AdapterError> {
        orders::take(device, &self.memo, order, Some(fiat_amount)).await
    }

    async fn pay_hold_invoice(
        &self,
        device: &DeviceContext<'_>,
        order: &OrderReference,
        path: PaymentPath,
    ) -> Result<PaymentHandoff, AdapterError> {
        payments::hold_invoice(
            device,
            &self.memo,
            order,
            path,
            self.config.message_window,
            self.poll,
        )
        .await
    }

    async fn buyer_invoice_amount(
        &self,
        device: &DeviceContext<'_>,
        order: &OrderReference,
    ) -> Result<u64, AdapterError> {
        payments::requested_payout(
            device,
            &self.memo,
            order,
            self.config.message_window,
            self.poll,
        )
        .await
    }

    async fn provide_buyer_invoice(
        &self,
        device: &DeviceContext<'_>,
        order: &OrderReference,
        invoice: Option<&Secret>,
    ) -> Result<Option<Secret>, AdapterError> {
        payments::provide(device, &self.memo, order, invoice).await
    }

    async fn submit_buyer_invoice(
        &self,
        device: &DeviceContext<'_>,
        order: &OrderReference,
        invoice: &Secret,
    ) -> Result<InvoiceSubmission, AdapterError> {
        payments::submit(device, &self.memo, order, invoice).await
    }

    async fn confirm_fiat_sent(
        &self,
        device: &DeviceContext<'_>,
        order: &OrderReference,
    ) -> Result<(), AdapterError> {
        trade::fiat_sent(device, order).await
    }

    async fn release(
        &self,
        device: &DeviceContext<'_>,
        order: &OrderReference,
    ) -> Result<(), AdapterError> {
        trade::release(device, order).await
    }

    async fn cancel_if_open(
        &self,
        device: &DeviceContext<'_>,
        order: &OrderReference,
    ) -> Result<CancelOutcome, AdapterError> {
        orders::cancel_if_open(device, order, self.config.message_window).await
    }

    async fn observe_order(
        &self,
        device: &DeviceContext<'_>,
        order: &OrderReference,
    ) -> Result<AppOrderState, AdapterError> {
        orders::observe(device, order, self.config.message_window).await
    }

    async fn visible_actions(
        &self,
        _device: &DeviceContext<'_>,
        _order: &OrderReference,
    ) -> Result<Vec<String>, AdapterError> {
        Err(AdapterError::contract(
            "mostro-cli offers every command at any time; it shows no actions",
        ))
    }

    async fn rate_counterpart(
        &self,
        device: &DeviceContext<'_>,
        order: &OrderReference,
        score: u8,
    ) -> Result<(), AdapterError> {
        trade::rate(device, order, score).await
    }

    async fn open_dispute(
        &self,
        device: &DeviceContext<'_>,
        order: &OrderReference,
    ) -> Result<(), AdapterError> {
        trade::open_dispute(device, order).await
    }

    async fn pay_bond_invoice(
        &self,
        device: &DeviceContext<'_>,
        order: &OrderReference,
        path: PaymentPath,
    ) -> Result<PaymentHandoff, AdapterError> {
        bond::pay(
            device,
            &self.memo,
            order,
            path,
            self.config.message_window,
            self.poll,
        )
        .await
    }

    async fn decline_bond(
        &self,
        device: &DeviceContext<'_>,
        order: &OrderReference,
    ) -> Result<(), AdapterError> {
        bond::decline(device, order).await
    }

    async fn bond_claim(
        &self,
        device: &DeviceContext<'_>,
        order: &OrderReference,
    ) -> Result<Option<BondClaim>, AdapterError> {
        bond::claim(device, &self.memo, order, self.config.message_window).await
    }

    async fn submit_bond_claim_invoice(
        &self,
        device: &DeviceContext<'_>,
        order: &OrderReference,
        invoice: &Secret,
    ) -> Result<String, AdapterError> {
        bond::submit_claim(
            device,
            &self.memo,
            order,
            invoice,
            self.config.message_window,
        )
        .await
    }

    async fn bond_slash_notice(
        &self,
        device: &DeviceContext<'_>,
        order: &OrderReference,
    ) -> Result<Option<String>, AdapterError> {
        bond::slash_notice(device, order, self.config.message_window).await
    }

    async fn cancel_payment(
        &self,
        device: &DeviceContext<'_>,
        order: &OrderReference,
    ) -> Result<(), AdapterError> {
        orders::cancel(device, order).await
    }
}

#[cfg(test)]
mod tests;
