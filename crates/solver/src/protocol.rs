//! The messages a solver sends and how the daemon's answers are read.
//! Pure: unit tested without a relay.

use mostro_core::message::{Action, Message, Payload};
use nostr_sdk::prelude::{PublicKey, ToBech32};
use uuid::Uuid;

/// How the daemon answered a solver request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reply {
    /// The expected confirmation.
    Done,
    /// A `cant-do` for this request, with the daemon's reason.
    Refused(String),
    /// A message about something else.
    Unrelated,
}

/// A fresh request id, echoed by the daemon in its answer.
fn request_id() -> u64 {
    let bytes = Uuid::new_v4().into_bytes();
    let mut id = [0u8; 8];
    id.copy_from_slice(&bytes[..8]);
    u64::from_be_bytes(id)
}

/// `admin-add-solver` for `solver`, read-write: the operator's act. The
/// daemon reads the solver out of the payload and looks nothing up by id,
/// but its message check still demands an id on this action and drops
/// the message without a word when it is missing, so a fresh one is set.
pub fn add_solver_message(solver: &PublicKey) -> Message {
    let npub = solver.to_bech32().unwrap_or_else(|_| solver.to_hex());
    Message::new_dispute(
        Some(Uuid::new_v4()),
        Some(request_id()),
        None,
        Action::AdminAddSolver,
        Some(Payload::TextMessage(format!("{npub}:read-write"))),
    )
}

/// `admin-take-dispute` for `dispute_id`: the message id is the dispute's.
pub fn take_dispute_message(dispute_id: Uuid) -> Message {
    Message::new_dispute(
        Some(dispute_id),
        Some(request_id()),
        None,
        Action::AdminTakeDispute,
        None,
    )
}

/// `admin-settle` or `admin-cancel` for `order_id`, on the dispute
/// wrapper: the message id is the order's (the daemon resolves the order
/// from it, unlike the take, which carries the dispute id), and the
/// dispute must already be this solver's.
pub fn ruling_message(order_id: Uuid, ruling: Action) -> Message {
    Message::new_dispute(Some(order_id), Some(request_id()), None, ruling, None)
}

/// Reads a daemon message as the answer to a request that expects
/// `expected` back. A `cant-do` counts only when it echoes the request id
/// (or carries none, as some refusals do); anything else is unrelated.
pub fn classify_reply(reply: &Message, expected: &Action, request_id: Option<u64>) -> Reply {
    let kind = reply.get_inner_message_kind();
    let same_request = kind.request_id.is_none() || kind.request_id == request_id;
    if &kind.action == expected && same_request {
        return Reply::Done;
    }
    if kind.action == Action::CantDo && same_request {
        let reason = match &kind.payload {
            Some(Payload::CantDo(Some(reason))) => format!("{reason:?}"),
            _ => "no reason given".to_owned(),
        };
        return Reply::Refused(reason);
    }
    Reply::Unrelated
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use mostro_core::error::CantDoReason;
    use mostro_core::prelude::{Transport, WrapOptions, unwrap_message_nip44, wrap_message_with};
    use nostr_sdk::prelude::Keys;

    #[test]
    fn add_solver_names_the_solver_npub_read_write() {
        let solver = Keys::generate();
        let message = add_solver_message(&solver.public_key());
        let kind = message.get_inner_message_kind();
        assert_eq!(kind.action, Action::AdminAddSolver);
        assert!(
            kind.id.is_some(),
            "the daemon drops an id-less admin message"
        );
        assert!(kind.verify());
        let npub = solver.public_key().to_bech32().unwrap();
        assert!(matches!(
            &kind.payload,
            Some(Payload::TextMessage(text)) if *text == format!("{npub}:read-write")
        ));
    }

    #[test]
    fn take_dispute_is_addressed_by_dispute_id_and_rulings_by_order_id() {
        let dispute = Uuid::new_v4();
        let order = Uuid::new_v4();
        let take = take_dispute_message(dispute);
        assert_eq!(take.get_inner_message_kind().id, Some(dispute));
        assert_eq!(
            take.get_inner_message_kind().action,
            Action::AdminTakeDispute
        );
        for ruling in [Action::AdminSettle, Action::AdminCancel] {
            let message = ruling_message(order, ruling.clone());
            assert_eq!(message.get_inner_message_kind().id, Some(order));
            assert_eq!(message.get_inner_message_kind().action, ruling);
            assert!(message.get_inner_message_kind().request_id.is_some());
            assert!(message.get_inner_message_kind().verify());
            assert!(matches!(message, Message::Dispute(_)));
        }
        assert_ne!(
            take.get_inner_message_kind().request_id,
            ruling_message(order, Action::AdminSettle)
                .get_inner_message_kind()
                .request_id
        );
    }

    #[test]
    fn replies_are_matched_by_action_and_request_id() {
        let done = Message::new_order(None, Some(7), None, Action::AdminSettled, None);
        assert_eq!(
            classify_reply(&done, &Action::AdminSettled, Some(7)),
            Reply::Done
        );
        assert_eq!(
            classify_reply(&done, &Action::AdminSettled, Some(8)),
            Reply::Unrelated
        );
        let unnumbered = Message::new_order(None, None, None, Action::AdminSettled, None);
        assert_eq!(
            classify_reply(&unnumbered, &Action::AdminSettled, Some(7)),
            Reply::Done
        );
        let refused = Message::new_order(
            None,
            Some(7),
            None,
            Action::CantDo,
            Some(Payload::CantDo(Some(CantDoReason::IsNotYourDispute))),
        );
        assert_eq!(
            classify_reply(&refused, &Action::AdminSettled, Some(7)),
            Reply::Refused("IsNotYourDispute".into())
        );
        let other = Message::new_order(None, Some(7), None, Action::AdminTookDispute, None);
        assert_eq!(
            classify_reply(&other, &Action::AdminSettled, Some(7)),
            Reply::Unrelated
        );
    }

    /// The wire round trip the solver relies on: a registration wrapped
    /// from the daemon key to itself opens on the daemon side with that
    /// identity, and the daemon's answer opens for the solver.
    #[tokio::test]
    async fn messages_wrap_and_unwrap_on_the_nip44_transport() {
        let daemon = Keys::generate();
        let solver = Keys::generate();
        let opts = || WrapOptions {
            pow: 0,
            expiration: None,
            signed: true,
        };
        let registration = wrap_message_with(
            Transport::Nip44Direct,
            &add_solver_message(&solver.public_key()),
            &daemon,
            &daemon,
            daemon.public_key(),
            opts(),
        )
        .await
        .unwrap();
        let opened = unwrap_message_nip44(&registration, &daemon)
            .unwrap()
            .unwrap();
        assert_eq!(opened.identity, daemon.public_key());
        assert_eq!(opened.sender, daemon.public_key());
        assert!(opened.signature.is_some());
        assert_eq!(
            opened.message.get_inner_message_kind().action,
            Action::AdminAddSolver
        );

        let answer = wrap_message_with(
            Transport::Nip44Direct,
            &Message::new_order(None, Some(1), None, Action::AdminSettled, None),
            &daemon,
            &daemon,
            solver.public_key(),
            opts(),
        )
        .await
        .unwrap();
        let opened = unwrap_message_nip44(&answer, &solver).unwrap().unwrap();
        assert_eq!(
            classify_reply(&opened.message, &Action::AdminSettled, Some(1)),
            Reply::Done
        );
        // Not addressed to this key: unreadable, never an error.
        assert!(unwrap_message_nip44(&answer, &daemon).unwrap().is_none());
    }
}
