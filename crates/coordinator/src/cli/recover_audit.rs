//! What the daemon published about orders during a run, read straight from
//! the relay. Operator attributions for a recovery journal are checked
//! against it, never trusted on their own.
use std::time::Duration;

use mortsom_app_contract::OrderState;
use mortsom_environment::daemon::host_view;
use nostr_sdk::prelude::*;

/// Kind of the daemon's public, replaceable order event.
const ORDER_EVENT_KIND: u16 = 38383;

/// The newest public status the daemon published for one order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuditedOrder {
    pub id: String,
    pub status: String,
    pub created_at: u64,
}

impl AuditedOrder {
    /// Whether the daemon closed the order (`canceled`, `success`, `expired`).
    pub fn is_closed(&self) -> bool {
        OrderState::from_protocol_status(&self.status).is_some_and(|state| state.is_terminal())
    }
}

/// The daemon's order events published at or after `since`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RelayAudit {
    pub since: u64,
    pub orders: Vec<AuditedOrder>,
}

impl RelayAudit {
    /// Orders the daemon published in the window that the journal does not
    /// already know, whatever their status: every candidate for what an
    /// unresolved submission may have created.
    pub fn unknown(&self, known: &[String]) -> Vec<&AuditedOrder> {
        self.orders
            .iter()
            .filter(|order| !known.contains(&order.id))
            .collect()
    }

    /// Orders the daemon created in the window that the journal does not
    /// already know and that are not closed: exactly what an "it produced no
    /// order" statement would leave behind.
    pub fn open_unknown(&self, known: &[String]) -> Vec<&AuditedOrder> {
        self.orders
            .iter()
            .filter(|order| !known.contains(&order.id) && !order.is_closed())
            .collect()
    }

    /// Whether the daemon published `id` in the window.
    pub fn published(&self, id: &str) -> bool {
        self.orders.iter().any(|order| order.id == id)
    }
}

/// Where and how to fetch the daemon's order events for a run's window, so
/// the same audit can be repeated later, after the retained clients ran.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RelayAuditSource {
    pub relay_url: String,
    pub daemon_pubkey: String,
    /// Unix seconds; the run's start.
    pub since: u64,
    pub timeout: Duration,
}

impl RelayAuditSource {
    /// The audit of `run`'s window against the stack it runs on: what a run
    /// checks after each desktop scenario and what a later cleanup repeats.
    pub fn for_run(
        stack: &crate::stack::StackSnapshot,
        run: &mortsom_common::RunId,
        timeout: Duration,
    ) -> Result<Self, String> {
        let since = run
            .started_at()
            .ok_or("cannot audit the relay: the run id carries no start time")?
            .timestamp()
            .max(0) as u64;
        Ok(Self {
            relay_url: stack.relay_url.clone(),
            daemon_pubkey: stack.daemon_pubkey.clone(),
            since,
            timeout,
        })
    }

    /// A fresh audit of the window, as of now.
    pub async fn fetch(&self) -> Result<RelayAudit, String> {
        fetch(
            &self.relay_url,
            &self.daemon_pubkey,
            self.since,
            self.timeout,
        )
        .await
    }
}

/// Fetches the daemon's order events since `since` (unix seconds) from
/// `relay_url`. One entry per order, its newest status.
pub async fn fetch(
    relay_url: &str,
    daemon_pubkey: &str,
    since: u64,
    timeout: Duration,
) -> Result<RelayAudit, String> {
    let pubkey = PublicKey::from_hex(daemon_pubkey).map_err(|e| format!("daemon pubkey: {e}"))?;
    let url = RelayUrl::parse(&host_view(relay_url)).map_err(|e| format!("relay url: {e}"))?;
    let client = Client::default();
    client
        .add_relay(url)
        .await
        .map_err(|e| format!("relay audit: cannot add relay: {e}"))?;
    client.connect().await;
    let filter = Filter::new()
        .author(pubkey)
        .kind(Kind::Custom(ORDER_EVENT_KIND))
        .since(Timestamp::from_secs(since));
    let events = client
        .fetch_events(filter)
        .timeout(timeout)
        .await
        .map_err(|e| format!("relay audit: fetch failed: {e}"))?;
    client.disconnect().await;
    let mut newest: std::collections::BTreeMap<String, AuditedOrder> = Default::default();
    for event in events {
        let Some(id) = event.tags.identifier() else {
            continue;
        };
        let status = event
            .tags
            .iter()
            .map(|t| t.as_slice())
            .find(|t| t.first().map(String::as_str) == Some("s"))
            .and_then(|t| t.get(1).cloned())
            .unwrap_or_default();
        let candidate = AuditedOrder {
            id: id.to_owned(),
            status,
            created_at: event.created_at.as_secs(),
        };
        let keep = newest
            .get(&candidate.id)
            .is_none_or(|current| current.created_at <= candidate.created_at);
        if keep {
            newest.insert(candidate.id.clone(), candidate);
        }
    }
    Ok(RelayAudit {
        since,
        orders: newest.into_values().collect(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn order(id: &str, status: &str) -> AuditedOrder {
        AuditedOrder {
            id: id.into(),
            status: status.into(),
            created_at: 10,
        }
    }

    #[test]
    fn open_unknown_orders_are_those_neither_known_nor_closed() {
        let audit = RelayAudit {
            since: 1,
            orders: vec![
                order("known", "pending"),
                order("expired", "expired"),
                order("canceled", "canceled"),
                order("done", "success"),
                order("live", "in-progress"),
                order("fresh", "pending"),
            ],
        };
        let open: Vec<&str> = audit
            .open_unknown(&["known".to_owned()])
            .iter()
            .map(|o| o.id.as_str())
            .collect();
        assert_eq!(open, ["live", "fresh"]);
        assert!(audit.published("expired") && !audit.published("other"));
        let unknown: Vec<&str> = audit
            .unknown(&["known".to_owned()])
            .iter()
            .map(|o| o.id.as_str())
            .collect();
        assert_eq!(unknown, ["expired", "canceled", "done", "live", "fresh"]);
    }
}
