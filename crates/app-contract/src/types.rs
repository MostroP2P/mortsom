//! Shared value types of the application contract.

use std::path::PathBuf;

use mortsom_common::{Lane, Secret};
use serde::{Deserialize, Serialize};

use crate::states::OrderState;

/// A named application profile (spec §12.2).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppProfile {
    /// Profile name (`mobile-legacy`, `mobile-v2`).
    pub name: String,
    /// Android application id.
    pub package: String,
}

/// Inputs to build the app.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuildContext {
    /// Source checkout.
    pub source_dir: PathBuf,
    /// Build flavor.
    pub flavor: String,
    /// Non-secret `KEY=VALUE` dart defines.
    pub dart_defines: Vec<String>,
}

/// A built (or supplied) APK.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Apk {
    /// File path.
    pub path: PathBuf,
    /// Application id.
    pub package: String,
}

/// The test environment an app instance must be pointed at.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TestEnvironment {
    /// Daemon public key (hex).
    pub mostro_pubkey: String,
    /// Relay URL as the emulator reaches it (`ws://10.0.2.2:<port>`).
    pub relay_url: String,
    /// Lane, shown in reports and in the app's environment marker.
    pub lane: Lane,
}

/// An onboarded identity as displayed by the app. Never the mnemonic.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Identity {
    /// Public key as shown (npub or hex).
    pub public_key: String,
}

/// A connected wallet, described without its URI.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WalletConnection {
    /// Opaque label (`alice`, `bob`).
    pub label: String,
    /// Whether the app displays a connected state.
    pub connected: bool,
}

/// Order side from the maker's perspective.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OrderKind {
    /// Maker sells sats.
    Sell,
    /// Maker buys sats.
    Buy,
}

/// What to create.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OrderDraft {
    /// Side.
    pub kind: OrderKind,
    /// Sats amount (fixed price).
    pub amount_sats: u64,
    /// Fiat currency code.
    pub fiat_code: String,
    /// Fiat amount.
    pub fiat_amount: u64,
    /// Payment method label.
    pub payment_method: String,
}

/// An order as the app displays it after creation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VisibleOrder {
    /// Order id when the app shows one.
    pub id: Option<String>,
    /// Normalized state.
    pub state: OrderState,
}

/// How a scenario refers to an order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OrderReference {
    /// Order id.
    pub id: String,
}

/// Normalized state plus the raw label the app rendered.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppOrderState {
    /// Normalized state.
    pub state: OrderState,
    /// Raw status label as displayed (for the timeline, not for assertions).
    pub raw_label: String,
}

/// Outcome of a payment step handed to the wallet path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PaymentHandoff {
    /// The app paid through the connected NWC wallet automatically.
    NwcAutomatic,
    /// The app displayed an invoice for an external wallet; the runner
    /// records only its payment hash, never the invoice text.
    ExternalInvoice {
        /// Invoice, protected because it may identify the counterpart.
        invoice: Secret,
    },
}

/// Outcome of `cancel_if_open`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CancelOutcome {
    /// The order was open and is now cancelled.
    Cancelled,
    /// The order was already in a terminal state.
    AlreadyClosed,
    /// The app does not show this order.
    NotFound,
}
