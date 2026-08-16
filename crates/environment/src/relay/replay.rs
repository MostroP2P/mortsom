//! Replaying signed events onto the run relay (spec §9.3): the runner never
//! forges daemon events, it re-publishes ones it already observed so the app
//! must cope with duplicate, stale and out-of-order deliveries.

use std::time::Duration;

use nostr_sdk::prelude::{Client, Event, RelayUrl};
use thiserror::Error;

/// Replay failures.
#[derive(Debug, Error)]
pub enum ReplayError {
    /// The relay URL is invalid.
    #[error("invalid relay URL {0}")]
    InvalidUrl(String),
    /// The relay could not be reached.
    #[error("relay {0} not connected")]
    NotConnected(String),
    /// A JSON payload was not a valid signed event.
    #[error("event {index} is not a valid signed event")]
    InvalidEvent {
        /// Position in the input.
        index: usize,
    },
    /// The relay rejected an event.
    #[error("relay rejected event {id}: {message}")]
    Rejected {
        /// Event id.
        id: String,
        /// Relay message.
        message: String,
    },
}

/// Publishes already-signed events (JSON) to `relay_url` in the given order.
/// Returns how many the relay accepted. Nothing is signed or altered.
pub async fn replay_events(
    relay_url: &str,
    events_json: &[String],
    timeout: Duration,
) -> Result<usize, ReplayError> {
    let url =
        RelayUrl::parse(relay_url).map_err(|_| ReplayError::InvalidUrl(relay_url.to_owned()))?;
    let mut parsed = Vec::with_capacity(events_json.len());
    for (index, json) in events_json.iter().enumerate() {
        let event = Event::from_json(json).map_err(|_| ReplayError::InvalidEvent { index })?;
        event
            .verify()
            .map_err(|_| ReplayError::InvalidEvent { index })?;
        parsed.push(event);
    }
    let client = Client::default();
    client
        .add_relay(url)
        .await
        .map_err(|_| ReplayError::InvalidUrl(relay_url.to_owned()))?;
    let output = client.try_connect().timeout(timeout).await;
    if output.success.is_empty() {
        return Err(ReplayError::NotConnected(relay_url.to_owned()));
    }
    let mut accepted = 0;
    for event in parsed {
        let id = event.id.to_hex();
        match tokio::time::timeout(timeout, client.send_event(&event)).await {
            // `send_event` succeeds even when every relay refused the
            // event; only an acceptance counts as a replayed event.
            Ok(Ok(output)) if !output.success.is_empty() => accepted += 1,
            Ok(Ok(_)) => {
                client.disconnect().await;
                return Err(ReplayError::Rejected {
                    id,
                    message: "no relay accepted the event".to_owned(),
                });
            }
            Ok(Err(e)) => {
                client.disconnect().await;
                return Err(ReplayError::Rejected {
                    id,
                    message: e.to_string(),
                });
            }
            Err(_) => {
                client.disconnect().await;
                return Err(ReplayError::Rejected {
                    id,
                    message: "send timed out".into(),
                });
            }
        }
    }
    client.disconnect().await;
    Ok(accepted)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn rejects_invalid_urls_and_events_before_connecting() {
        assert!(matches!(
            replay_events("not a url", &[], Duration::from_millis(100)).await,
            Err(ReplayError::InvalidUrl(_))
        ));
        assert!(matches!(
            replay_events(
                "ws://127.0.0.1:1",
                &["{}".into()],
                Duration::from_millis(100)
            )
            .await,
            Err(ReplayError::InvalidEvent { index: 0 })
        ));
        assert!(matches!(
            replay_events("ws://127.0.0.1:1", &[], Duration::from_millis(200)).await,
            Err(ReplayError::NotConnected(_))
        ));
    }
}
