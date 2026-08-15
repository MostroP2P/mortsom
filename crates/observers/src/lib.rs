//! Mortsom observers: read-only UI, notification, relay, wallet and artifact
//! evidence sources.

pub mod relay;

pub use relay::{ObservedEvent, ObserverError, ObserverScope, RelayObserver};
