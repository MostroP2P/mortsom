//! A dispute solver for one run (spec §8.5). The daemon lets only a
//! registered solver rule on a dispute, and registering one is the
//! operator's act, signed with the daemon's own key. Mortsom owns the
//! stack, so it holds that key: each run mints a fresh solver identity,
//! registers it, and rules through the protocol exactly as an operator's
//! solver client would. The scenario sees a [`DisputeSolver`]; the keys
//! never leave this crate.

use std::time::Duration;

use async_trait::async_trait;
use futures::StreamExt;
use mortsom_common::Secret;
use mortsom_scenario_kit::context::{BondSlash, DisputeSolver};
use mostro_core::message::{Action, BondResolution, Message, Payload};
use mostro_core::prelude::{Transport, WrapOptions, unwrap_message_nip44, wrap_message_with};
use nostr_sdk::prelude::{
    Client, ClientNotification, Event, Filter, Keys, Kind, PublicKey, RelayUrl,
};
use thiserror::Error;
use tracing::{debug, info};
use uuid::Uuid;

mod protocol;
pub use protocol::{
    Reply, add_solver_message, classify_reply, ruling_message, ruling_message_with,
    take_dispute_message,
};

/// Solver failures. Messages never carry key material.
#[derive(Debug, Error)]
pub enum SolverError {
    /// The relay could not be reached.
    #[error("solver cannot connect to {relay}: {detail}")]
    Connect {
        /// Relay dialled.
        relay: String,
        /// Why.
        detail: String,
    },
    /// The daemon key in the stack settings is unusable.
    #[error("the daemon key is not a usable nsec")]
    DaemonKey,
    /// A protocol message could not be built or sent.
    #[error("solver cannot send {action:?}: {detail}")]
    Send {
        /// What was being sent.
        action: Action,
        /// Why.
        detail: String,
    },
    /// The daemon refused the request.
    #[error("the daemon refused {action:?}: {reason}")]
    Refused {
        /// What was refused.
        action: Action,
        /// The daemon's reason.
        reason: String,
    },
    /// No answer within the deadline.
    #[error("no answer from the daemon to {action:?} within {timeout:?}")]
    Timeout {
        /// What went unanswered.
        action: Action,
        /// The deadline.
        timeout: Duration,
    },
}

/// A solver identity connected to the run relay.
pub struct Solver {
    client: Client,
    keys: Keys,
    daemon: PublicKey,
    timeout: Duration,
}

impl Solver {
    /// Mints a solver identity and connects it to `relay_url`. Nothing is
    /// sent until [`Solver::register`].
    pub async fn connect(
        relay_url: &str,
        daemon: PublicKey,
        timeout: Duration,
    ) -> Result<Self, SolverError> {
        let client = Client::default();
        let url = RelayUrl::parse(relay_url).map_err(|e| SolverError::Connect {
            relay: relay_url.to_owned(),
            detail: e.to_string(),
        })?;
        client
            .add_relay(url.clone())
            .await
            .map_err(|e| SolverError::Connect {
                relay: relay_url.to_owned(),
                detail: e.to_string(),
            })?;
        let output = client.try_connect().timeout(timeout).await;
        if !output.success.keys().any(|u| *u == url) {
            let detail = output
                .failed
                .get(&url)
                .map(ToString::to_string)
                .unwrap_or_else(|| "not connected".into());
            return Err(SolverError::Connect {
                relay: relay_url.to_owned(),
                detail,
            });
        }
        let keys = Keys::generate();
        // Live delivery of everything the daemon sends this identity; the
        // subscription is in place before any request goes out.
        client
            .subscribe(
                Filter::new()
                    .kind(Kind::PrivateDirectMessage)
                    .author(daemon)
                    .pubkey(keys.public_key())
                    .limit(0),
            )
            .await
            .map_err(|e| SolverError::Connect {
                relay: relay_url.to_owned(),
                detail: e.to_string(),
            })?;
        Ok(Self {
            client,
            keys,
            daemon,
            timeout,
        })
    }

    /// The solver's public key, for the observer scope.
    pub fn public_key(&self) -> PublicKey {
        self.keys.public_key()
    }

    /// Registers this solver with the daemon, as its operator would: an
    /// `admin-add-solver` sent from the daemon's own key, identity and
    /// trade key alike, which is the only key the daemon accepts it from.
    /// The daemon's acknowledgement goes to that key, so it is not
    /// awaited: a registration that did not land shows up as the first
    /// ruling being refused for an unknown public key.
    pub async fn register(&self, daemon_nsec: &Secret) -> Result<(), SolverError> {
        let daemon_keys = Keys::parse(daemon_nsec.expose()).map_err(|_| SolverError::DaemonKey)?;
        if daemon_keys.public_key() != self.daemon {
            return Err(SolverError::DaemonKey);
        }
        let message = add_solver_message(&self.keys.public_key());
        let event = wrap(&message, &daemon_keys, &daemon_keys, self.daemon).await?;
        self.client
            .send_event(&event)
            .await
            .map_err(|e| SolverError::Send {
                action: Action::AdminAddSolver,
                detail: e.to_string(),
            })?;
        info!(solver = %self.keys.public_key(), "solver registration sent to the daemon");
        Ok(())
    }

    /// Sends `message` as this solver and waits for the daemon's answer:
    /// the reply carrying `expected`, or its refusal.
    /// The solver is one fixed identity: identity key and trade key alike.
    async fn request(&self, message: Message, expected: Action) -> Result<Message, SolverError> {
        let action = message.get_inner_message_kind().action.clone();
        let request_id = message.get_inner_message_kind().request_id;
        let mut notifications = self.client.notifications();
        let event = wrap(&message, &self.keys, &self.keys, self.daemon).await?;
        self.client
            .send_event(&event)
            .await
            .map_err(|e| SolverError::Send {
                action: action.clone(),
                detail: e.to_string(),
            })?;
        debug!(?action, "solver request sent");
        let deadline = tokio::time::Instant::now() + self.timeout;
        loop {
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            if remaining.is_zero() {
                return Err(SolverError::Timeout {
                    action,
                    timeout: self.timeout,
                });
            }
            let notification = match tokio::time::timeout(remaining, notifications.next()).await {
                Ok(Some(notification)) => notification,
                Ok(None) | Err(_) => {
                    return Err(SolverError::Timeout {
                        action,
                        timeout: self.timeout,
                    });
                }
            };
            let ClientNotification::Event { event, .. } = notification else {
                continue;
            };
            let Some(reply) = self.open(&event) else {
                continue;
            };
            match classify_reply(&reply, &expected, request_id) {
                Reply::Done => return Ok(reply),
                Reply::Refused(reason) => {
                    return Err(SolverError::Refused { action, reason });
                }
                Reply::Unrelated => {}
            }
        }
    }

    /// Decrypts a daemon message addressed to this solver, if it is one.
    fn open(&self, event: &Event) -> Option<Message> {
        if event.kind != Kind::PrivateDirectMessage || event.pubkey != self.daemon {
            return None;
        }
        match unwrap_message_nip44(event, &self.keys) {
            Ok(Some(unwrapped)) => Some(unwrapped.message),
            Ok(None) => None,
            Err(e) => {
                debug!(error = %e, "daemon message to the solver could not be opened");
                None
            }
        }
    }

    /// Disconnects.
    pub async fn stop(&self) {
        self.client.disconnect().await;
    }
}

async fn wrap(
    message: &Message,
    identity: &Keys,
    trade: &Keys,
    receiver: PublicKey,
) -> Result<Event, SolverError> {
    wrap_message_with(
        Transport::Nip44Direct,
        message,
        identity,
        trade,
        receiver,
        WrapOptions {
            pow: 0,
            expiration: None,
            signed: true,
        },
    )
    .await
    .map_err(|e| SolverError::Send {
        action: message.get_inner_message_kind().action.clone(),
        detail: e.to_string(),
    })
}

#[async_trait]
impl DisputeSolver for Solver {
    async fn take_dispute(&self, dispute_id: &str) -> Result<String, String> {
        let id = Uuid::parse_str(dispute_id).map_err(|e| format!("dispute id: {e}"))?;
        let reply = self
            .request(take_dispute_message(id), Action::AdminTookDispute)
            .await
            .map_err(|e| e.to_string())?;
        match &reply.get_inner_message_kind().payload {
            Some(Payload::Dispute(_, Some(info))) => Ok(info.id.to_string()),
            other => Err(format!(
                "the daemon's admin-took-dispute carried no dispute info: {other:?}"
            )),
        }
    }

    async fn settle(&self, order_id: &str) -> Result<(), String> {
        let id = Uuid::parse_str(order_id).map_err(|e| format!("order id: {e}"))?;
        self.request(
            ruling_message(id, Action::AdminSettle),
            Action::AdminSettled,
        )
        .await
        .map(|_| ())
        .map_err(|e| e.to_string())
    }

    async fn cancel(&self, order_id: &str) -> Result<(), String> {
        let id = Uuid::parse_str(order_id).map_err(|e| format!("order id: {e}"))?;
        self.request(
            ruling_message(id, Action::AdminCancel),
            Action::AdminCanceled,
        )
        .await
        .map(|_| ())
        .map_err(|e| e.to_string())
    }

    async fn settle_slashing(&self, order_id: &str, slash: BondSlash) -> Result<(), String> {
        let id = Uuid::parse_str(order_id).map_err(|e| format!("order id: {e}"))?;
        self.request(
            ruling_message_with(id, Action::AdminSettle, Some(resolution(slash))),
            Action::AdminSettled,
        )
        .await
        .map(|_| ())
        .map_err(|e| e.to_string())
    }

    async fn cancel_slashing(&self, order_id: &str, slash: BondSlash) -> Result<(), String> {
        let id = Uuid::parse_str(order_id).map_err(|e| format!("order id: {e}"))?;
        self.request(
            ruling_message_with(id, Action::AdminCancel, Some(resolution(slash))),
            Action::AdminCanceled,
        )
        .await
        .map(|_| ())
        .map_err(|e| e.to_string())
    }
}

/// The daemon's shape of a slash decision.
fn resolution(slash: BondSlash) -> BondResolution {
    BondResolution {
        slash_seller: slash.seller,
        slash_buyer: slash.buyer,
    }
}
