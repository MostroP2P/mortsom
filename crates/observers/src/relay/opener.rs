//! Opening the actors' protocol messages to the daemon under test, on a
//! stack Mortsom owns and so holds the daemon key of. Only the action is
//! kept -- never the content -- to tell a read-only query an app makes on
//! any launch from a trade action an interruption must not repeat.

use mostro_core::prelude::{Action, Message, unwrap_message_nip44};
use nostr_sdk::prelude::{Event, Keys};

/// What an opened actor message asked of the daemon.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Opened {
    /// The protocol action, as the wire names it (`fiat-sent`, `user-info`).
    pub action: String,
    /// True when it only reads the daemon's record and changes nothing.
    pub query: bool,
}

/// Opens `event` with the daemon's keys; `None` when it is not addressed
/// to this daemon or does not parse.
pub fn open(event: &Event, daemon: &Keys) -> Option<Opened> {
    let unwrapped = unwrap_message_nip44(event, daemon).ok().flatten()?;
    let kind = unwrapped.message.get_inner_message_kind();
    Some(Opened {
        action: wire_name(&kind.action),
        query: is_query(&unwrapped.message, &kind.action),
    })
}

/// The action as the protocol writes it (`user-info`); its `Display` is the
/// Rust variant name.
fn wire_name(action: &Action) -> String {
    serde_json::to_value(action)
        .ok()
        .and_then(|value| value.as_str().map(str::to_owned))
        .unwrap_or_else(|| action.to_string())
}

/// Requests that read the daemon's record and change nothing: the restore
/// channel (a session restore, the user's own reputation) and the lookups
/// an app makes to rebuild what it shows.
fn is_query(message: &Message, action: &Action) -> bool {
    matches!(message, Message::Restore(_))
        || matches!(
            action,
            Action::Orders | Action::LastTradeIndex | Action::RestoreSession | Action::UserInfo
        )
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use mostro_core::prelude::{MessageKind, Transport, WrapOptions, wrap_message_with};

    async fn sent(message: &Message, daemon: &Keys) -> Event {
        let trade = Keys::generate();
        let options = WrapOptions {
            pow: 0,
            expiration: None,
            signed: true,
        };
        wrap_message_with(
            Transport::Nip44Direct,
            message,
            &trade,
            &trade,
            daemon.public_key(),
            options,
        )
        .await
        .unwrap()
    }

    #[tokio::test]
    async fn the_apps_own_reputation_request_is_a_query() {
        let daemon = Keys::generate();
        // As app v2 builds it: the restore wrapper, the user-info action.
        let user_info = Message::Restore(MessageKind::new(
            None,
            Some(7),
            None,
            Action::UserInfo,
            None,
        ));

        let opened = open(&sent(&user_info, &daemon).await, &daemon).unwrap();

        assert_eq!(opened.action, "user-info");
        assert!(opened.query);
    }

    #[tokio::test]
    async fn a_trade_step_is_an_action() {
        let daemon = Keys::generate();
        let fiat_sent = Message::new_order(None, Some(7), Some(1), Action::FiatSent, None);

        let opened = open(&sent(&fiat_sent, &daemon).await, &daemon).unwrap();

        assert_eq!(opened.action, "fiat-sent");
        assert!(!opened.query);
    }

    #[tokio::test]
    async fn an_order_lookup_is_a_query() {
        let daemon = Keys::generate();
        let lookup = Message::new_order(None, Some(7), None, Action::Orders, None);

        assert!(open(&sent(&lookup, &daemon).await, &daemon).unwrap().query);
    }

    #[tokio::test]
    async fn a_message_to_another_daemon_does_not_open() {
        let daemon = Keys::generate();
        let fiat_sent = Message::new_order(None, Some(7), Some(1), Action::FiatSent, None);

        let event = sent(&fiat_sent, &Keys::generate()).await;

        assert_eq!(open(&event, &daemon), None);
    }
}
