//! Mortsom application contract (spec §12.1): the `AppAdapter` trait,
//! normalized business states, capabilities and shared types every adapter
//! implements and the scenario kit consumes. Nothing here names a widget,
//! a Flutter route, a Rust-core type or a localized string.

pub mod adapter;
pub mod capabilities;
pub mod config;
pub mod error;
pub mod states;
pub mod types;

pub use adapter::{AppAdapter, DeviceContext};
pub use capabilities::{Capability, CapabilitySet};
pub use config::MobileConfig;
pub use error::AdapterError;
pub use states::OrderState;
pub use types::{
    AppOrderState, AppProfile, CancelOutcome, FiatAmount, Identity, InvoiceSubmission, OrderDraft,
    OrderKind, OrderPrice, OrderReference, PaymentHandoff, PaymentPath, TestEnvironment,
    VisibleOrder, WalletConnection,
};
