//! The `AppAdapter` trait (spec §12.1): business capabilities rather than
//! widget details. One implementation per application generation.

use std::time::Duration;

use async_trait::async_trait;
use mortsom_common::Actor;
use mortsom_common::Secret;
use mortsom_ui_driver::ui::UiSession;

use crate::capabilities::CapabilitySet;
use crate::error::AdapterError;
use crate::types::{
    AppOrderState, AppProfile, CancelOutcome, Identity, InvoiceSubmission, OrderDraft,
    OrderReference, PaymentHandoff, PaymentPath, TestEnvironment, VisibleOrder, WalletConnection,
};

/// A client as seen by an adapter: one actor, one UI session, one deadline
/// for any single UI wait.
pub struct DeviceContext<'a> {
    /// Actor operating this device.
    pub actor: Actor,
    /// UI automation session.
    pub ui: &'a dyn UiSession,
    /// Default deadline for one UI wait.
    pub ui_timeout: Duration,
}

/// Business capabilities of one application generation.
#[async_trait]
pub trait AppAdapter: Send + Sync {
    /// The profile this adapter drives.
    fn profile(&self) -> AppProfile;
    /// What this profile supports.
    fn capabilities(&self) -> CapabilitySet;
    /// Drives first-launch onboarding with a fresh generated identity and
    /// returns the identity as displayed.
    async fn onboard(
        &self,
        device: &DeviceContext<'_>,
        env: &TestEnvironment,
    ) -> Result<Identity, AdapterError>;
    /// Reads the identity currently displayed (after a restart, for example).
    async fn displayed_identity(
        &self,
        device: &DeviceContext<'_>,
    ) -> Result<Identity, AdapterError>;
    /// Configures the custom Mostro node and the test relay through settings.
    async fn configure_node_and_relay(
        &self,
        device: &DeviceContext<'_>,
        env: &TestEnvironment,
    ) -> Result<(), AdapterError>;
    /// Configures NWC through the settings UI and returns the connected state.
    async fn configure_nwc(
        &self,
        device: &DeviceContext<'_>,
        label: &str,
        uri: &Secret,
    ) -> Result<WalletConnection, AdapterError>;
    /// Existing identity read without generation or onboarding (recovery only).
    async fn existing_identity(
        &self,
        _device: &DeviceContext<'_>,
    ) -> Result<Identity, AdapterError> {
        Err(AdapterError::contract(
            "read-only identity recovery is not supported",
        ))
    }
    /// Order IDs displayed in the actor's own trades list.
    async fn owned_orders(&self, _device: &DeviceContext<'_>) -> Result<Vec<String>, AdapterError> {
        Err(AdapterError::contract(
            "own-order discovery is not supported",
        ))
    }
    /// Creates a fixed-price order.
    async fn create_order(
        &self,
        device: &DeviceContext<'_>,
        draft: &OrderDraft,
    ) -> Result<VisibleOrder, AdapterError>;
    /// Finds an order in the book and takes it.
    async fn find_and_take_order(
        &self,
        device: &DeviceContext<'_>,
        order: &OrderReference,
    ) -> Result<(), AdapterError>;
    /// Finds a range order in the book and takes it at `fiat_amount`, which
    /// the app asks for before the take. A fixed order never asks.
    async fn find_and_take_order_at(
        &self,
        _device: &DeviceContext<'_>,
        _order: &OrderReference,
        _fiat_amount: u64,
    ) -> Result<(), AdapterError> {
        Err(AdapterError::contract(
            "taking a range order at an amount is not supported",
        ))
    }
    /// Hands off the hold invoice along `path`.
    ///
    /// On [`PaymentPath::Nwc`] the app pays it with its connected wallet and
    /// the returned invoice is only for hash correlation. On
    /// [`PaymentPath::ExternalWallet`] the app is left waiting on the pay
    /// screen and the returned invoice is the thing the caller must pay —
    /// which is how a run drives a node Mortsom holds rather than a wallet
    /// the app holds.
    async fn pay_hold_invoice(
        &self,
        device: &DeviceContext<'_>,
        order: &OrderReference,
        path: PaymentPath,
    ) -> Result<PaymentHandoff, AdapterError>;
    /// Opens the buyer invoice form and reads the actual requested sats.
    /// Implementations must not generate or submit an invoice during this read.
    async fn buyer_invoice_amount(
        &self,
        _device: &DeviceContext<'_>,
        _order: &OrderReference,
    ) -> Result<u64, AdapterError> {
        Err(AdapterError::contract(
            "buyer invoice amount readout is not supported",
        ))
    }
    /// Submits the buyer invoice and returns it (protected) when the app
    /// exposed it, for hash correlation.
    ///
    /// `None` submits through the app's connected wallet. `Some(invoice)`
    /// types one the caller issued into the app's manual form, for a run
    /// whose wallet is a node Mortsom holds.
    async fn provide_buyer_invoice(
        &self,
        device: &DeviceContext<'_>,
        order: &OrderReference,
        invoice: Option<&Secret>,
    ) -> Result<Option<Secret>, AdapterError>;
    /// Types `invoice` into the manual form, submits it once and reports
    /// the daemon's verdict: the trade detail (accepted) or the form kept
    /// open with the reason on screen (rejected). Unlike
    /// [`AppAdapter::provide_buyer_invoice`], a refusal is an answer here,
    /// not a failure: scenarios about invoices the daemon cannot pay ask
    /// exactly that question.
    async fn submit_buyer_invoice(
        &self,
        _device: &DeviceContext<'_>,
        _order: &OrderReference,
        _invoice: &Secret,
    ) -> Result<InvoiceSubmission, AdapterError> {
        Err(AdapterError::contract(
            "buyer invoice submission with a verdict is not supported",
        ))
    }
    /// Reports fiat sent.
    async fn confirm_fiat_sent(
        &self,
        device: &DeviceContext<'_>,
        order: &OrderReference,
    ) -> Result<(), AdapterError>;
    /// Releases the sats.
    async fn release(
        &self,
        device: &DeviceContext<'_>,
        order: &OrderReference,
    ) -> Result<(), AdapterError>;
    /// Cancels the order if it is still open. Idempotent.
    async fn cancel_if_open(
        &self,
        device: &DeviceContext<'_>,
        order: &OrderReference,
    ) -> Result<CancelOutcome, AdapterError>;
    /// Reads the order's displayed state.
    async fn observe_order(
        &self,
        device: &DeviceContext<'_>,
        order: &OrderReference,
    ) -> Result<AppOrderState, AdapterError>;
    /// Reads the order's state from its detail screen only, leaving any
    /// invoice or payment form that is open. A form reports a fixed waiting
    /// state for as long as it is on screen, so a wait for the daemon to
    /// *change* something must not read it; the detail reflects the change.
    /// Adapters without such a distinction read the same thing as
    /// [`Self::observe_order`].
    async fn observe_order_detail(
        &self,
        device: &DeviceContext<'_>,
        order: &OrderReference,
    ) -> Result<AppOrderState, AdapterError> {
        self.observe_order(device, order).await
    }
    /// Names of the trade actions the app currently offers for `order`
    /// (normalized: `pay-invoice`, `add-invoice`, `fiat-sent`, `release`,
    /// `cancel`, `dispute`, `rate`), used by unauthorized-action checks.
    async fn visible_actions(
        &self,
        device: &DeviceContext<'_>,
        order: &OrderReference,
    ) -> Result<Vec<String>, AdapterError>;
    /// Rates the counterpart of a completed order with `score` stars (1-5)
    /// and returns once the app shows the trade as rated.
    async fn rate_counterpart(
        &self,
        _device: &DeviceContext<'_>,
        _order: &OrderReference,
        _score: u8,
    ) -> Result<(), AdapterError> {
        Err(AdapterError::contract(
            "rating a counterpart is not supported",
        ))
    }
    /// Opens a dispute on an active or fiat-sent order as this actor and
    /// returns once the app has accepted the request.
    async fn open_dispute(
        &self,
        _device: &DeviceContext<'_>,
        _order: &OrderReference,
    ) -> Result<(), AdapterError> {
        Err(AdapterError::contract("opening a dispute is not supported"))
    }
    /// Cancels the order from the pay-invoice screen instead of paying.
    async fn cancel_payment(
        &self,
        device: &DeviceContext<'_>,
        order: &OrderReference,
    ) -> Result<(), AdapterError>;
}
