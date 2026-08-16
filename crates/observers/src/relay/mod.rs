//! Relay observer (spec §4.3, Phase 1.4): a read-only subscription scoped to
//! the daemon under test, the run's actor identities and run-created order
//! ids. Events are deduplicated, sequenced and kept as an ordered log that
//! the coordinator persists as redacted JSONL evidence.

mod scope;

use std::collections::HashSet;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures::StreamExt;
use nostr_sdk::prelude::{Client, ClientNotification, Event, RelayUrl, SubscriptionId};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio::sync::broadcast;
use tokio::task::JoinHandle;
use tracing::{debug, warn};

pub use scope::ObserverScope;

/// Broadcast capacity; a slow consumer that lags only misses live pushes,
/// the ordered log keeps everything.
const CHANNEL_CAPACITY: usize = 1024;

/// Stable subscription id prefix; each scope filter gets `<prefix>-<n>` so a
/// refresh replaces the previous subscriptions instead of accumulating.
const SUBSCRIPTION_PREFIX: &str = "mortsom-observer";

/// NIP-17 direct message kind (protocol v2 transport).
const PRIVATE_DM_KIND: u16 = 14;

/// One observed relay event, normalized for evidence. Encrypted content
/// (kind 14) is never stored; public content is kept as-is and redacted at
/// the artifact boundary.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ObservedEvent {
    /// Position in the run's ordered log, starting at 1.
    pub seq: u64,
    /// Wall-clock arrival time (unix milliseconds).
    pub received_at_ms: u64,
    /// Relay the event arrived from.
    pub relay: String,
    /// Event id (hex).
    pub id: String,
    /// Event kind.
    pub kind: u16,
    /// Author public key (hex).
    pub author: String,
    /// Event creation time (unix seconds).
    pub created_at: u64,
    /// Tags as string arrays.
    pub tags: Vec<Vec<String>>,
    /// Content for public kinds; `None` for encrypted direct messages.
    pub content: Option<String>,
    /// The signed event as JSON for public kinds (replayable evidence);
    /// `None` for direct messages.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub raw: Option<String>,
}

impl ObservedEvent {
    /// First value of the first tag named `name`.
    pub fn tag(&self, name: &str) -> Option<&str> {
        self.tags
            .iter()
            .find(|t| t.first().map(String::as_str) == Some(name))
            .and_then(|t| t.get(1))
            .map(String::as_str)
    }
}

/// Observer failures.
#[derive(Debug, Error)]
pub enum ObserverError {
    /// A relay URL could not be parsed or added.
    #[error("invalid relay {url}: {message}")]
    Relay {
        /// Offending URL.
        url: String,
        /// Detail.
        message: String,
    },
    /// No relay accepted the connection.
    #[error("no relay connected within {0:?}")]
    NotConnected(Duration),
    /// Subscribing failed.
    #[error("subscribe failed: {0}")]
    Subscribe(String),
}

struct Inner {
    client: Client,
    /// False for detached observers (no relay): subscriptions are skipped
    /// and events arrive only through `ingest`.
    live: bool,
    scope: Mutex<ObserverScope>,
    log: Mutex<Vec<ObservedEvent>>,
    seen: Mutex<HashSet<String>>,
    subscriptions: Mutex<Vec<SubscriptionId>>,
    /// Serialises `resubscribe`: the subscription ids are stable, so two
    /// refreshes running at once would unsubscribe each other's work and
    /// disagree about what is live.
    refresh: tokio::sync::Mutex<()>,
    tx: broadcast::Sender<ObservedEvent>,
}

/// A live relay observer. Cloning shares the same log and subscription.
#[derive(Clone)]
pub struct RelayObserver {
    inner: Arc<Inner>,
    task: Arc<Mutex<Option<JoinHandle<()>>>>,
}

impl RelayObserver {
    /// Connects to `relay_urls`, subscribes for `scope` and starts ingesting.
    pub async fn start(
        relay_urls: &[String],
        scope: ObserverScope,
        connect_timeout: Duration,
    ) -> Result<Self, ObserverError> {
        let client = Client::default();
        for url in relay_urls {
            let relay = RelayUrl::parse(url).map_err(|e| ObserverError::Relay {
                url: url.clone(),
                message: e.to_string(),
            })?;
            client
                .add_relay(relay)
                .await
                .map_err(|e| ObserverError::Relay {
                    url: url.clone(),
                    message: e.to_string(),
                })?;
        }
        let output = client.try_connect().timeout(connect_timeout).await;
        if output.success.is_empty() {
            return Err(ObserverError::NotConnected(connect_timeout));
        }
        let mut notifications = client.notifications();
        let (tx, _) = broadcast::channel(CHANNEL_CAPACITY);
        let inner = Arc::new(Inner {
            client,
            live: true,
            scope: Mutex::new(scope),
            log: Mutex::new(Vec::new()),
            seen: Mutex::new(HashSet::new()),
            subscriptions: Mutex::new(Vec::new()),
            refresh: tokio::sync::Mutex::new(()),
            tx,
        });
        let observer = Self {
            inner: Arc::clone(&inner),
            task: Arc::new(Mutex::new(None)),
        };
        observer.resubscribe().await?;
        let ingest_target = observer.clone();
        let task = tokio::spawn(async move {
            while let Some(notification) = notifications.next().await {
                if let ClientNotification::Event {
                    relay_url, event, ..
                } = notification
                {
                    ingest_target.ingest(&relay_url.to_string(), &event);
                }
            }
        });
        *lock(&observer.task) = Some(task);
        Ok(observer)
    }

    /// An observer without a relay connection: events arrive only through
    /// [`RelayObserver::ingest`]. Used by fakes and tests of consumers.
    pub fn detached(scope: ObserverScope) -> Self {
        let (tx, _) = broadcast::channel(CHANNEL_CAPACITY);
        Self {
            inner: Arc::new(Inner {
                client: Client::default(),
                live: false,
                scope: Mutex::new(scope),
                log: Mutex::new(Vec::new()),
                seen: Mutex::new(HashSet::new()),
                subscriptions: Mutex::new(Vec::new()),
                refresh: tokio::sync::Mutex::new(()),
                tx,
            }),
            task: Arc::new(Mutex::new(None)),
        }
    }

    /// Adds a run-created order id to the scope and refreshes the
    /// subscription so its events are observed from now on.
    pub async fn add_order(&self, order_id: &str) -> Result<(), ObserverError> {
        lock(&self.inner.scope).add_order(order_id);
        if !self.inner.live {
            return Ok(());
        }
        self.resubscribe().await
    }

    /// Adds an actor identity to the scope (direct messages to and from it
    /// are observed as metadata only).
    pub async fn add_actor(
        &self,
        pubkey: nostr_sdk::prelude::PublicKey,
    ) -> Result<(), ObserverError> {
        lock(&self.inner.scope).add_actor(pubkey);
        if !self.inner.live {
            return Ok(());
        }
        self.resubscribe().await
    }

    /// Replaces the current subscriptions with the scope's filter set. Ids
    /// are stable, so nothing accumulates on the relay across refreshes.
    async fn resubscribe(&self) -> Result<(), ObserverError> {
        // Held for the whole refresh: `add_order` and `add_actor` can be
        // called concurrently, and interleaving them would leave the relay
        // subscribed to something nobody is tracking.
        let _refresh = self.inner.refresh.lock().await;
        let previous: Vec<SubscriptionId> = std::mem::take(&mut *lock(&self.inner.subscriptions));
        for id in &previous {
            if let Err(e) = self.inner.client.unsubscribe(id).await {
                // The relay may already be gone; the id is dropped either way.
                debug!(subscription = %id, error = %e, "unsubscribe failed");
            }
        }
        let filters = lock(&self.inner.scope).filters();
        let mut current = Vec::with_capacity(filters.len());
        for (index, filter) in filters.into_iter().enumerate() {
            let id = SubscriptionId::new(format!("{SUBSCRIPTION_PREFIX}-{index}"));
            if let Err(e) = self
                .inner
                .client
                .subscribe(filter)
                .with_id(id.clone())
                .await
            {
                // Whatever was created before the failure is live on the
                // relay; forgetting it here would leak it past teardown.
                *lock(&self.inner.subscriptions) = current;
                return Err(ObserverError::Subscribe(e.to_string()));
            }
            current.push(id);
        }
        *lock(&self.inner.subscriptions) = current;
        Ok(())
    }

    /// Subscription ids currently held (stable across refreshes).
    pub fn subscription_ids(&self) -> Vec<SubscriptionId> {
        lock(&self.inner.subscriptions).clone()
    }

    /// Ingests one event: scope check, dedupe, sequence, log, broadcast.
    /// Returns the logged event when it was new and in scope.
    pub fn ingest(&self, relay: &str, event: &Event) -> Option<ObservedEvent> {
        if !lock(&self.inner.scope).matches(event) {
            debug!(id = %event.id, kind = %event.kind, "event outside observer scope");
            return None;
        }
        let mut log = lock(&self.inner.log);
        let id = event.id.to_hex();
        if !lock(&self.inner.seen).insert(id.clone()) {
            return None;
        }
        let observed = ObservedEvent {
            seq: log.len() as u64 + 1,
            received_at_ms: now_ms(),
            relay: relay.to_owned(),
            id,
            kind: event.kind.as_u16(),
            author: event.pubkey.to_hex(),
            created_at: event.created_at.as_secs(),
            tags: event.tags.iter().map(|t| t.as_slice().to_vec()).collect(),
            content: scope::keeps_content(event.kind).then(|| event.content.clone()),
            raw: scope::keeps_content(event.kind).then(|| event.as_json()),
        };
        log.push(observed.clone());
        drop(log);
        // No live receiver is not an error: the ordered log is the record,
        // the broadcast is only a wake-up for `wait_for`.
        let _ = self.inner.tx.send(observed.clone());
        Some(observed)
    }

    /// Live stream of newly logged events.
    pub fn subscribe(&self) -> broadcast::Receiver<ObservedEvent> {
        self.inner.tx.subscribe()
    }

    /// Ordered copy of everything observed so far.
    pub fn snapshot(&self) -> Vec<ObservedEvent> {
        lock(&self.inner.log).clone()
    }

    /// Sequence number of the last logged event (0 when empty); a baseline
    /// for "nothing new after this point" observations.
    pub fn last_seq(&self) -> u64 {
        lock(&self.inner.log).last().map_or(0, |e| e.seq)
    }

    /// Protocol messages (kind 14) sent by anyone other than the daemon
    /// after `seq`: on an isolated run relay these are the actors' actions.
    pub fn actor_messages_since(&self, seq: u64) -> Vec<ObservedEvent> {
        let daemon = lock(&self.inner.scope).mostro_pubkey.to_hex();
        lock(&self.inner.log)
            .iter()
            .filter(|e| e.seq > seq && e.kind == PRIVATE_DM_KIND && e.author != daemon)
            .cloned()
            .collect()
    }

    /// Waits until an event satisfying `predicate` exists in the log or
    /// arrives, bounded by `timeout`. Never sleeps blindly.
    pub async fn wait_for<P>(&self, timeout: Duration, predicate: P) -> Option<ObservedEvent>
    where
        P: Fn(&ObservedEvent) -> bool,
    {
        let mut rx = self.subscribe();
        if let Some(found) = self.snapshot().into_iter().find(|e| predicate(e)) {
            return Some(found);
        }
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            if remaining.is_zero() {
                return None;
            }
            match tokio::time::timeout(remaining, rx.recv()).await {
                Ok(Ok(event)) if predicate(&event) => return Some(event),
                Ok(Ok(_)) => {}
                Ok(Err(broadcast::error::RecvError::Lagged(n))) => {
                    warn!(missed = n, "observer consumer lagged; rechecking the log");
                    if let Some(found) = self.snapshot().into_iter().find(|e| predicate(e)) {
                        return Some(found);
                    }
                }
                Ok(Err(broadcast::error::RecvError::Closed)) | Err(_) => return None,
            }
        }
    }

    /// Closes the subscriptions, stops ingesting and disconnects.
    pub async fn stop(&self) {
        if let Some(task) = lock(&self.task).take() {
            task.abort();
        }
        if let Err(e) = self.inner.client.unsubscribe_all().await {
            debug!(error = %e, "unsubscribe_all failed during stop");
        }
        self.inner.client.disconnect().await;
    }
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    // The log is evidence: recover it even after a panic elsewhere.
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Wall-clock milliseconds; a clock before the epoch degrades to zero rather
/// than aborting evidence collection.
fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}

#[cfg(test)]
mod tests;
