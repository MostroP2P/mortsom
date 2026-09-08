//! Injectable connection boundary for the run relay observer.
use super::OBSERVER_CONNECT_TIMEOUT;
use mortsom_observers::relay::{ObserverScope, RelayObserver};
use std::sync::Arc;

/// Starts the relay observer for a run (real: connects; tests: detached).
#[async_trait::async_trait]
pub trait ObserverFactory: Send + Sync {
    /// Starts observing `relay_url` for the daemon `pubkey`.
    async fn start(
        &self,
        relay_url: &str,
        pubkey: nostr_sdk::prelude::PublicKey,
    ) -> Result<Arc<RelayObserver>, String>;
}

/// Connects to the run relay.
#[derive(Debug, Default, Clone, Copy)]
pub struct LiveObserver;

#[async_trait::async_trait]
impl ObserverFactory for LiveObserver {
    async fn start(
        &self,
        relay_url: &str,
        pubkey: nostr_sdk::prelude::PublicKey,
    ) -> Result<Arc<RelayObserver>, String> {
        RelayObserver::start(
            &[relay_url.to_owned()],
            ObserverScope::daemon_only(pubkey),
            OBSERVER_CONNECT_TIMEOUT,
        )
        .await
        .map(Arc::new)
        .map_err(|e| e.to_string())
    }
}
