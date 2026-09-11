//! Shared value types of the application contract.

use std::fmt;

use mortsom_common::{Lane, Secret};
use serde::{Deserialize, Serialize};

use crate::states::OrderState;

/// A named application profile (spec §12.2).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppProfile {
    /// Profile name (`mobile-v2`).
    pub name: String,
}

/// The test environment an app instance must be pointed at.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TestEnvironment {
    /// Daemon public key (hex).
    pub mostro_pubkey: String,
    /// Relay URL as the client reaches it (`ws://127.0.0.1:<port>`).
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

/// How the fiat side of an order is sized.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum FiatAmount {
    /// One fiat amount.
    Exact(u64),
    /// A range the taker picks an amount from; the daemon spawns the
    /// remainder as a new order once the take completes.
    Range {
        /// Smallest amount a taker may pick.
        min: u64,
        /// Largest amount a taker may pick.
        max: u64,
    },
}

impl fmt::Display for FiatAmount {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FiatAmount::Exact(amount) => write!(f, "{amount}"),
            FiatAmount::Range { min, max } => write!(f, "{min}-{max}"),
        }
    }
}

/// How the sats side of an order is priced.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum OrderPrice {
    /// The maker names the sats.
    Fixed {
        /// Sats amount.
        amount_sats: u64,
    },
    /// The daemon resolves the sats from the exchange rate when the order
    /// is taken, adjusted by a whole-percent premium.
    Market {
        /// Premium over the market rate, in percent.
        premium: i32,
    },
}

impl fmt::Display for OrderPrice {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            OrderPrice::Fixed { amount_sats } => write!(f, "{amount_sats} sats"),
            OrderPrice::Market { premium } => write!(f, "market {premium:+}%"),
        }
    }
}

/// What to create.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OrderDraft {
    /// Side.
    pub kind: OrderKind,
    /// Fiat currency code.
    pub fiat_code: String,
    /// Fiat amount, exact or a range.
    pub fiat: FiatAmount,
    /// Payment method label.
    pub payment_method: String,
    /// Sats pricing.
    pub price: OrderPrice,
}

impl OrderDraft {
    /// A fixed-price order for one fiat amount: the ordinary trade.
    pub fn fixed(
        kind: OrderKind,
        amount_sats: u64,
        fiat_code: impl Into<String>,
        fiat_amount: u64,
        payment_method: impl Into<String>,
    ) -> Self {
        Self {
            kind,
            fiat_code: fiat_code.into(),
            fiat: FiatAmount::Exact(fiat_amount),
            payment_method: payment_method.into(),
            price: OrderPrice::Fixed { amount_sats },
        }
    }

    /// A market-priced order for one fiat amount, at no premium.
    pub fn market(
        kind: OrderKind,
        fiat_code: impl Into<String>,
        fiat_amount: u64,
        payment_method: impl Into<String>,
    ) -> Self {
        Self {
            kind,
            fiat_code: fiat_code.into(),
            fiat: FiatAmount::Exact(fiat_amount),
            payment_method: payment_method.into(),
            price: OrderPrice::Market { premium: 0 },
        }
    }

    /// A range order. The protocol prices ranges at market only, so the
    /// price is market at no premium.
    pub fn range(
        kind: OrderKind,
        fiat_code: impl Into<String>,
        min: u64,
        max: u64,
        payment_method: impl Into<String>,
    ) -> Self {
        Self {
            kind,
            fiat_code: fiat_code.into(),
            fiat: FiatAmount::Range { min, max },
            payment_method: payment_method.into(),
            price: OrderPrice::Market { premium: 0 },
        }
    }

    /// Whether the taker picks the amount.
    pub fn is_range(&self) -> bool {
        matches!(self.fiat, FiatAmount::Range { .. })
    }
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
    /// The maker's side, when known; helps a taker find the right book tab.
    #[serde(default)]
    pub kind: Option<OrderKind>,
    /// Whether the daemon sizes the sats (market price or range) rather
    /// than the maker: the amounts it asks for are then bounded by the
    /// per-order cap, not by the run's fixed amount.
    #[serde(default)]
    pub priced_by_daemon: bool,
}

impl OrderReference {
    /// Reference by id only.
    pub fn new(id: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            kind: None,
            priced_by_daemon: false,
        }
    }

    /// Reference with the maker's side.
    pub fn with_kind(id: impl Into<String>, kind: OrderKind) -> Self {
        Self {
            id: id.into(),
            kind: Some(kind),
            priced_by_daemon: false,
        }
    }

    /// Reference to an order created from `draft`.
    pub fn from_draft(id: impl Into<String>, draft: &OrderDraft) -> Self {
        Self {
            id: id.into(),
            kind: Some(draft.kind),
            priced_by_daemon: matches!(draft.price, OrderPrice::Market { .. }),
        }
    }
}

/// Normalized state plus the raw label the app rendered.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppOrderState {
    /// Normalized state.
    pub state: OrderState,
    /// Raw status label as displayed (for the timeline, not for assertions).
    pub raw_label: String,
}

/// Outcome of a payment step handed to the wallet path. The invoice is
/// protected: only its payment hash is ever recorded. Callers must register
/// the invoice with the run redactor as soon as they receive it, because
/// the app renders it in the accessibility tree that evidence dumps
/// capture.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PaymentHandoff {
    /// How the payment was made.
    pub kind: PaymentPath,
    /// The invoice the app displayed, if it exposed one.
    pub invoice: Option<Secret>,
}

/// Which path the app used to pay.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PaymentPath {
    /// The app paid through the connected NWC wallet.
    Nwc,
    /// The app handed the invoice to an external wallet.
    ExternalWallet,
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

/// The daemon's verdict on a buyer invoice submitted through the app.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InvoiceSubmission {
    /// The daemon took the invoice: the app returned to the trade detail.
    Accepted,
    /// The daemon refused it and the app kept the form open, showing why.
    Rejected {
        /// The reason the app displays, as its stable readout.
        reason: String,
    },
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod draft_tests {
    use super::*;

    #[test]
    fn the_three_draft_shapes_name_their_pricing() {
        let fixed = OrderDraft::fixed(OrderKind::Sell, 1000, "USD", 1, "cash");
        assert_eq!(fixed.price, OrderPrice::Fixed { amount_sats: 1000 });
        assert_eq!(fixed.fiat, FiatAmount::Exact(1));
        assert!(!fixed.is_range());

        let market = OrderDraft::market(OrderKind::Buy, "USD", 2, "cash");
        assert_eq!(market.price, OrderPrice::Market { premium: 0 });
        assert!(!market.is_range());

        let range = OrderDraft::range(OrderKind::Sell, "USD", 1, 3, "cash");
        assert_eq!(range.fiat, FiatAmount::Range { min: 1, max: 3 });
        assert_eq!(range.price, OrderPrice::Market { premium: 0 });
        assert!(range.is_range());
    }

    #[test]
    fn a_reference_remembers_who_prices_the_order() {
        let fixed = OrderDraft::fixed(OrderKind::Sell, 1000, "USD", 1, "cash");
        assert!(!OrderReference::from_draft("o1", &fixed).priced_by_daemon);
        let market = OrderDraft::market(OrderKind::Sell, "USD", 1, "cash");
        assert!(OrderReference::from_draft("o1", &market).priced_by_daemon);
        assert!(!OrderReference::new("o1").priced_by_daemon);
    }

    #[test]
    fn drafts_render_for_the_timeline_and_round_trip_through_json() {
        let range = OrderDraft::range(OrderKind::Sell, "USD", 1, 3, "cash");
        assert_eq!(range.fiat.to_string(), "1-3");
        assert_eq!(range.price.to_string(), "market +0%");
        assert_eq!(
            OrderPrice::Fixed { amount_sats: 1000 }.to_string(),
            "1000 sats"
        );
        let json = serde_json::to_string(&range).unwrap();
        assert!(json.contains("\"range\""), "{json}");
        let back: OrderDraft = serde_json::from_str(&json).unwrap();
        assert_eq!(back, range);
    }
}
