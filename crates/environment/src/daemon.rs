//! Read-only observation of the external Mostro daemon: its kind-38385 info
//! event carries version, protocol, timeouts and Lightning node status.
//! Nothing here creates an order or sends a message.

use std::collections::BTreeMap;
use std::time::Duration;

use mostro_core::prelude::NOSTR_INFO_EVENT_KIND;
use nostr_sdk::prelude::{Client, Event, Filter, Kind, PublicKey, RelayUrl};
use serde::{Deserialize, Serialize};
use tracing::debug;

/// Hostname the daemon uses to reach the host from inside Docker.
pub const DOCKER_HOST_ALIAS: &str = "host.docker.internal";
/// The daemon republishes its info event every five minutes by default
/// (`publish_mostro_info_interval = 300`); older than this is suspicious.
pub const INFO_STALE_AFTER: Duration = Duration::from_secs(30 * 60);

/// The daemon's self-published information, decoded from a kind-38385 event.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DaemonInfo {
    /// Author of the event, i.e. the daemon identity.
    pub pubkey: String,
    /// Event creation time (unix seconds).
    pub created_at: u64,
    /// Every single-value tag, keyed by tag name.
    pub tags: BTreeMap<String, String>,
}

impl DaemonInfo {
    /// Decodes an info event. Returns `None` for any other kind. Signature
    /// verification is performed by the relay pipeline of `nostr-sdk`
    /// before an event is handed to callers, so `pubkey` is trustworthy.
    pub fn from_event(event: &Event) -> Option<Self> {
        if event.kind != Kind::Custom(NOSTR_INFO_EVENT_KIND) {
            return None;
        }
        let tags = event
            .tags
            .iter()
            .filter_map(|tag| {
                let slice = tag.as_slice();
                Some((slice.first()?.clone(), slice.get(1)?.clone()))
            })
            .collect();
        Some(Self {
            pubkey: event.pubkey.to_hex(),
            created_at: event.created_at.as_secs(),
            tags,
        })
    }

    /// A tag value, if present.
    pub fn tag(&self, name: &str) -> Option<&str> {
        self.tags.get(name).map(String::as_str)
    }

    /// A tag parsed as an unsigned integer.
    pub fn tag_u64(&self, name: &str) -> Option<u64> {
        self.tag(name)?.trim().parse().ok()
    }

    /// Protocol version advertised by the daemon.
    pub fn protocol_version(&self) -> Option<u64> {
        self.tag_u64("protocol_version")
    }

    /// True when the daemon reports a connected Lightning node.
    pub fn has_lightning_node(&self) -> bool {
        self.tag("lnd_node_pubkey")
            .is_some_and(|value| !value.is_empty())
    }

    /// Age of the event relative to `now_unix`, saturating at zero.
    pub fn age(&self, now_unix: u64) -> Duration {
        Duration::from_secs(now_unix.saturating_sub(self.created_at))
    }

    /// True when the event is older than [`INFO_STALE_AFTER`].
    pub fn is_stale(&self, now_unix: u64) -> bool {
        self.age(now_unix) > INFO_STALE_AFTER
    }
}

/// The daemon's NIP-65 relay list (kind 10002), decoded.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RelayList {
    /// Event creation time (unix seconds).
    pub created_at: u64,
    /// Relay URLs advertised (`r` tags), normalized without trailing slash.
    pub relays: Vec<String>,
}

impl RelayList {
    /// Decodes a kind-10002 event. Returns `None` for any other kind.
    pub fn from_event(event: &Event) -> Option<Self> {
        if event.kind != Kind::RelayList {
            return None;
        }
        let relays = event
            .tags
            .iter()
            .filter_map(|tag| {
                let slice = tag.as_slice();
                (slice.first()? == "r")
                    .then(|| slice.get(1).cloned())
                    .flatten()
            })
            .map(|url| url.trim_end_matches('/').to_owned())
            .collect();
        Some(Self {
            created_at: event.created_at.as_secs(),
            relays,
        })
    }

    /// True when any of `urls` is advertised, treating host aliases as equal.
    pub fn advertises_any(&self, urls: &[String]) -> bool {
        crate::relay_url::any_same_relay(&self.relays, urls)
    }
}

/// Outcome of probing the daemon through its relays.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DaemonProbe {
    /// Per-relay connection outcome.
    pub relays: Vec<RelayProbe>,
    /// The freshest info event found, if any.
    pub info: Option<DaemonInfo>,
    /// The freshest relay list found, if any.
    pub relay_list: Option<RelayList>,
}

/// One relay's connection outcome, as seen from the host.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RelayProbe {
    /// URL as configured (daemon's view).
    pub configured: String,
    /// URL actually dialled from the host.
    pub dialled: String,
    /// `None` when connected, otherwise the failure.
    pub error: Option<String>,
}

/// Rewrites a relay URL from the daemon's Docker view to the host's view.
pub fn host_view(url: &str) -> String {
    url.replace(DOCKER_HOST_ALIAS, "127.0.0.1")
}

/// Connects to `relays` (translated to the host view) and fetches the
/// daemon's newest info event. Read-only. The whole probe is bounded by
/// twice `timeout` (connect, then fetch).
pub async fn probe_daemon(relays: &[String], pubkey: PublicKey, timeout: Duration) -> DaemonProbe {
    let ceiling = timeout.saturating_mul(2);
    match tokio::time::timeout(ceiling, probe_daemon_unbounded(relays, pubkey, timeout)).await {
        Ok(probe) => probe,
        Err(_) => DaemonProbe {
            relays: relays
                .iter()
                .map(|configured| RelayProbe {
                    configured: configured.clone(),
                    dialled: host_view(configured),
                    error: Some(format!("probe timed out after {ceiling:?}")),
                })
                .collect(),
            info: None,
            relay_list: None,
        },
    }
}

async fn probe_daemon_unbounded(
    relays: &[String],
    pubkey: PublicKey,
    timeout: Duration,
) -> DaemonProbe {
    let client = Client::default();
    let mut probes = Vec::new();
    for configured in relays {
        // Normalize through `RelayUrl` so the string compared against the
        // connection report is exactly the one the client uses.
        let (dialled, error) = match RelayUrl::parse(&host_view(configured)) {
            Ok(url) => {
                let error = client
                    .add_relay(url.clone())
                    .await
                    .err()
                    .map(|e| e.to_string());
                (url.to_string(), error)
            }
            Err(e) => (
                host_view(configured),
                Some(format!("invalid relay URL: {e}")),
            ),
        };
        probes.push(RelayProbe {
            configured: configured.clone(),
            dialled,
            error,
        });
    }
    let output = client.try_connect().timeout(timeout).await;
    for probe in probes.iter_mut().filter(|p| p.error.is_none()) {
        let failed = output
            .failed
            .iter()
            .find(|(url, _)| url.to_string() == probe.dialled);
        match failed {
            Some((_, error)) => probe.error = Some(error.to_string()),
            None if !output
                .success
                .keys()
                .any(|url| url.to_string() == probe.dialled) =>
            {
                probe.error = Some("not connected".to_owned());
            }
            None => {}
        }
    }
    let (info, relay_list) = if probes.iter().any(|p| p.error.is_none()) {
        (
            fetch_info(&client, pubkey, timeout).await,
            fetch_relay_list(&client, pubkey, timeout).await,
        )
    } else {
        (None, None)
    };
    client.disconnect().await;
    DaemonProbe {
        relays: probes,
        info,
        relay_list,
    }
}

async fn fetch_relay_list(
    client: &Client,
    pubkey: PublicKey,
    timeout: Duration,
) -> Option<RelayList> {
    let filter = Filter::new().author(pubkey).kind(Kind::RelayList).limit(1);
    let events = match client.fetch_events(filter).timeout(timeout).await {
        Ok(events) => events,
        Err(e) => {
            debug!(error = %e, "relay list fetch failed");
            return None;
        }
    };
    events
        .into_iter()
        .max_by_key(|event| event.created_at)
        .and_then(|event| RelayList::from_event(&event))
}

async fn fetch_info(client: &Client, pubkey: PublicKey, timeout: Duration) -> Option<DaemonInfo> {
    let filter = Filter::new()
        .author(pubkey)
        .kind(Kind::Custom(NOSTR_INFO_EVENT_KIND))
        .limit(1);
    let events = match client.fetch_events(filter).timeout(timeout).await {
        Ok(events) => events,
        Err(e) => {
            debug!(error = %e, "info event fetch failed");
            return None;
        }
    };
    events
        .into_iter()
        .max_by_key(|event| event.created_at)
        .and_then(|event| DaemonInfo::from_event(&event))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use nostr_sdk::prelude::{EventBuilder, FinalizeEvent, Keys, Tag};

    fn info_event(tags: &[(&str, &str)]) -> Event {
        let keys = Keys::generate();
        let tags: Vec<Tag> = tags
            .iter()
            .map(|(k, v)| Tag::parse([*k, *v]).unwrap())
            .collect();
        EventBuilder::new(Kind::Custom(NOSTR_INFO_EVENT_KIND), "")
            .tags(tags)
            .finalize(&keys)
            .unwrap()
    }

    #[test]
    fn decodes_tags_from_an_info_event() {
        let event = info_event(&[
            ("protocol_version", "2"),
            ("lnd_node_pubkey", "02abc"),
            ("expiration_seconds", "900"),
        ]);

        let info = DaemonInfo::from_event(&event).unwrap();

        assert_eq!(info.protocol_version(), Some(2));
        assert!(info.has_lightning_node());
        assert_eq!(info.tag_u64("expiration_seconds"), Some(900));
        assert_eq!(info.pubkey, event.pubkey.to_hex());
    }

    #[test]
    fn ignores_events_of_other_kinds() {
        let keys = Keys::generate();
        let event = EventBuilder::new(Kind::TextNote, "hello")
            .finalize(&keys)
            .unwrap();

        assert!(DaemonInfo::from_event(&event).is_none());
    }

    #[test]
    fn missing_lightning_pubkey_means_no_node() {
        let event = info_event(&[("lnd_node_pubkey", ""), ("protocol_version", "x")]);

        let info = DaemonInfo::from_event(&event).unwrap();

        assert!(!info.has_lightning_node());
        assert_eq!(info.protocol_version(), None);
    }

    #[test]
    fn decodes_relay_list_and_matches_normalized_urls() {
        let keys = Keys::generate();
        let event = EventBuilder::new(Kind::RelayList, "")
            .tags([
                Tag::parse(["r", "ws://host.docker.internal:7500/"]).unwrap(),
                Tag::parse(["r", "wss://relay.example", "read"]).unwrap(),
                Tag::parse(["x", "ignored"]).unwrap(),
            ])
            .finalize(&keys)
            .unwrap();

        let list = RelayList::from_event(&event).unwrap();

        assert_eq!(
            list.relays,
            vec!["ws://host.docker.internal:7500", "wss://relay.example"]
        );
        assert!(list.advertises_any(&["ws://host.docker.internal:7500".into()]));
        assert!(
            list.advertises_any(&["ws://127.0.0.1:7500".into()]),
            "host alias"
        );
        assert!(!list.advertises_any(&["ws://127.0.0.1:7001".into()]));
        assert!(RelayList::from_event(&info_event(&[])).is_none());
    }

    #[test]
    fn staleness_is_measured_against_now() {
        let event = info_event(&[("protocol_version", "2")]);
        let info = DaemonInfo::from_event(&event).unwrap();
        let now = info.created_at;

        assert!(!info.is_stale(now + 60));
        assert!(info.is_stale(now + INFO_STALE_AFTER.as_secs() + 1));
        assert_eq!(info.age(now.saturating_sub(5)), Duration::ZERO);
    }

    #[test]
    fn host_view_rewrites_docker_alias() {
        assert_eq!(
            host_view("ws://host.docker.internal:7500"),
            "ws://127.0.0.1:7500"
        );
        assert_eq!(host_view("wss://relay.example"), "wss://relay.example");
    }

    #[tokio::test]
    async fn probe_reports_unreachable_relay_without_info() {
        let keys = Keys::generate();

        let probe = probe_daemon(
            &["ws://127.0.0.1:1".into(), "not a url".into()],
            keys.public_key(),
            Duration::from_millis(500),
        )
        .await;

        assert_eq!(probe.relays.len(), 2);
        assert!(probe.relays.iter().all(|r| r.error.is_some()), "{probe:?}");
        assert!(probe.info.is_none());
    }
}
