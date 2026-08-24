//! Mortsom observers: read-only UI, notification, relay, wallet and artifact
//! evidence sources.

pub mod relay;
pub mod wallet;

pub use relay::{ObservedEvent, ObserverError, ObserverScope, RelayObserver};
pub use wallet::{
    LndReader, PaymentObservation, PaymentState, WalletObserver, WalletReader, payment_hash_of,
};
