//! What the node asked an actor to do next on an order, read with
//! `message list`: the requests that arrive while the actor is idle (a hold
//! invoice for the maker, a payout request after the seller paid, a bond, a
//! slashed bond's share).
//! A list may come back truncated (`data.truncated`): what it holds is read
//! as usual, and what is missing from it proves nothing.

use std::time::Duration;

use mortsom_app_contract::{AdapterError, DeviceContext};
use mortsom_cli_driver::{Call, Envelope, unsigned_of};
use mortsom_common::{FailureCategory, Secret};
use serde_json::Value;
use tokio::time::Instant;

use crate::access::{missing, run};

/// How far back `message list` reads, in minutes: longer than any scenario.
const HISTORY_MINUTES: &str = "1440";

/// `next_step` of a hold invoice to pay.
pub(crate) const PAY_HOLD: &str = "pay_hold_invoice";
/// `next_step` of a payout invoice to hand in.
pub(crate) const ADD_INVOICE: &str = "add_invoice";
/// `next_step` of an anti-abuse bond to pay.
pub(crate) const PAY_BOND: &str = "pay_bond_invoice";

/// One request of the node.
#[derive(Debug, Clone)]
pub(crate) struct Request {
    /// The invoice to pay, when the request carries one.
    pub invoice: Option<Secret>,
    /// Sats asked for: the payload's amount, else its order's.
    pub amount_sats: Option<u64>,
    /// Unix seconds the node sent it.
    pub created_at: i64,
}

/// What `message list` returned for one order.
#[derive(Debug)]
pub(crate) struct Inbox {
    /// The node's messages, oldest first.
    pub messages: Vec<Value>,
    /// `data.truncated`: a relay returned its per-relay limit, or more than
    /// the CLI's record cap matched. The list may then be incomplete, so an
    /// action absent from it proves nothing.
    pub truncated: bool,
}

impl Inbox {
    /// Backs a verdict that rests on `what` being absent: `Ok` on a complete
    /// list, a relay failure on one that may be incomplete.
    pub(crate) fn prove_absence(&self, order_id: &str, what: &str) -> Result<(), AdapterError> {
        if self.truncated {
            return Err(inconclusive(order_id, what));
        }
        Ok(())
    }
}

/// The relay failure of a verdict that a truncated list cannot back.
pub(crate) fn inconclusive(order_id: &str, what: &str) -> AdapterError {
    AdapterError::new(
        FailureCategory::Relay,
        format!(
            "`message list` for order {order_id} may be incomplete (`truncated`), \
             so the absence of {what} proves nothing"
        ),
    )
}

/// The messages and the completeness flag of a `message list` outcome;
/// the contract promises both.
fn inbox_of(outcome: &Envelope) -> Result<Inbox, AdapterError> {
    let messages = outcome
        .field(&["messages"])
        .and_then(Value::as_array)
        .cloned()
        .ok_or_else(|| missing("message.list", "messages"))?;
    let truncated = outcome
        .field(&["truncated"])
        .and_then(Value::as_bool)
        .ok_or_else(|| missing("message.list", "truncated"))?;
    Ok(Inbox {
        messages,
        truncated,
    })
}

/// The messages the node sent this actor about `order_id`, oldest first,
/// and whether the list may be incomplete.
pub(crate) async fn list(
    device: &DeviceContext<'_>,
    order_id: &str,
) -> Result<Inbox, AdapterError> {
    let call = Call::node(&["message", "list"])
        .arg("order", order_id)
        .arg("since", HISTORY_MINUTES);
    inbox_of(&run(device, call).await?)
}

/// The newest request with `next_step` among `steps` sent after `after`.
pub(crate) fn newest(messages: &[Value], steps: &[&str], after: Option<i64>) -> Option<Request> {
    messages
        .iter()
        .rev()
        .filter_map(|message| request_of(message, steps))
        .find(|request| after.is_none_or(|after| request.created_at > after))
}

fn request_of(message: &Value, steps: &[&str]) -> Option<Request> {
    let step = message.get("next_step")?.as_str()?;
    if !steps.contains(&step) {
        return None;
    }
    let payload = message.get("payload").unwrap_or(&Value::Null);
    let amount = payload
        .get("amount")
        .and_then(unsigned_of)
        .or_else(|| {
            payload
                .get("order")
                .and_then(|order| order.get("sats_amount"))
                .and_then(unsigned_of)
        })
        .filter(|sats| *sats > 0);
    let invoice = payload
        .get("invoice")
        .and_then(Value::as_str)
        .map(Secret::new);
    // The node repeats a step with other payloads (a `pay-invoice` naming
    // the peer, for one): only a message carrying what its step needs is
    // the request itself.
    let complete = match step {
        PAY_HOLD | PAY_BOND => invoice.is_some(),
        _ => amount.is_some(),
    };
    if !complete {
        return None;
    }
    Some(Request {
        invoice,
        amount_sats: amount,
        created_at: message
            .get("created_at_unix")
            .and_then(unsigned_of)
            .and_then(|secs| i64::try_from(secs).ok())
            .unwrap_or(0),
    })
}

/// Waits, within the device's deadline, for a request among `steps` newer
/// than `after`. A truncated list is read again like one without the
/// request; a deadline that passes before any read ends, or after such a
/// read, proves nothing about the node, so it is a relay failure.
pub(crate) async fn wait_for(
    device: &DeviceContext<'_>,
    order_id: &str,
    steps: &[&str],
    after: Option<i64>,
    poll: Duration,
) -> Result<Request, AdapterError> {
    let deadline = Instant::now() + device.ui_timeout;
    // Whether the last read without the request was truncated; `None`
    // until a read ends.
    let mut truncated = None;
    let waited = tokio::time::timeout_at(deadline, async {
        loop {
            let inbox = list(device, order_id).await?;
            if let Some(request) = newest(&inbox.messages, steps, after) {
                return Ok(request);
            }
            truncated = Some(inbox.truncated);
            tokio::time::sleep(poll).await;
        }
    })
    .await;
    // A read cut short by the deadline is abandoned whole: the CLI driver
    // reaps its child and clears its runtime marker on its own.
    waited.unwrap_or_else(|_| Err(never_asked(order_id, steps, device.ui_timeout, truncated)))
}

/// The failure of a wait for `steps` that ran out after `waited`: the node
/// never asked, unless the last read was truncated and the request may be
/// beyond it.
/// The failure of a wait for `steps` that ran out after `waited`: the node
/// never asked only when the last read without the request was complete
/// (`Some(false)`); with no read ended, or a truncated one, nothing proves it.
fn never_asked(
    order_id: &str,
    steps: &[&str],
    waited: Duration,
    truncated: Option<bool>,
) -> AdapterError {
    let what = steps.join(" or ");
    match truncated {
        None => AdapterError::new(
            FailureCategory::Relay,
            format!(
                "no `message list` read for order {order_id} ended within {waited:?}, \
                 so nothing shows whether the node asked for {what}"
            ),
        ),
        Some(true) => inconclusive(order_id, &format!("a request for {what} within {waited:?}")),
        Some(false) => AdapterError::product(format!(
            "the node did not ask for {what} on order {order_id} within {waited:?}"
        )),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use serde_json::json;

    fn message(step: &str, at: &str, payload: Value) -> Value {
        json!({"action": "x", "next_step": step, "created_at_unix": at, "payload": payload})
    }

    #[test]
    fn the_newest_matching_request_wins_and_carries_its_invoice() {
        let messages = vec![
            message(
                PAY_HOLD,
                "10",
                json!({"kind": "payment_request", "invoice": "lnbcrt1old", "amount": null, "order": null}),
            ),
            message(
                ADD_INVOICE,
                "11",
                json!({"kind": "order", "order": {"sats_amount": "9900"}}),
            ),
            message(
                PAY_HOLD,
                "12",
                json!({"kind": "payment_request", "invoice": "lnbcrt1new", "amount": "10000", "order": null}),
            ),
        ];

        let hold = newest(&messages, &[PAY_HOLD], None).unwrap();
        let payout = newest(&messages, &[ADD_INVOICE], None).unwrap();

        assert_eq!(hold.invoice.unwrap().expose(), "lnbcrt1new");
        assert_eq!(hold.amount_sats, Some(10000));
        assert_eq!(payout.amount_sats, Some(9900));
        assert_eq!(payout.created_at, 11);
    }

    #[test]
    fn a_repeat_of_the_step_without_the_invoice_is_not_the_request() {
        let messages = vec![
            message(
                PAY_HOLD,
                "10",
                json!({"kind": "payment_request", "invoice": "lnbcrt1hold", "amount": null, "order": null}),
            ),
            message(
                PAY_HOLD,
                "11",
                json!({"kind": "peer", "counterparty_pubkey": "ab"}),
            ),
            message(ADD_INVOICE, "12", json!({"kind": "none"})),
        ];

        let hold = newest(&messages, &[PAY_HOLD], None).unwrap();

        assert_eq!(hold.invoice.unwrap().expose(), "lnbcrt1hold");
        assert!(newest(&messages, &[ADD_INVOICE], None).is_none());
    }

    #[test]
    fn a_request_older_than_the_last_submission_is_ignored() {
        let messages = vec![message(
            ADD_INVOICE,
            "11",
            json!({"kind": "order", "order": {"sats_amount": "9900"}}),
        )];

        assert!(newest(&messages, &[ADD_INVOICE], Some(11)).is_none());
        assert!(newest(&messages, &[ADD_INVOICE], Some(10)).is_some());
        assert!(newest(&messages, &[PAY_BOND], None).is_none());
    }

    fn outcome(data: Value) -> Envelope {
        let output = mortsom_cli_driver::testing::document("message.list", "ok", &data, None, 0);
        Envelope::parse(&output.stdout, output.code).unwrap()
    }

    #[test]
    fn a_listing_says_whether_it_may_be_incomplete() {
        let complete = outcome(json!({"messages": [{"action": "rate"}], "truncated": false}));
        let truncated = outcome(json!({"messages": [], "truncated": true}));

        let complete = inbox_of(&complete).unwrap();
        let truncated = inbox_of(&truncated).unwrap();

        assert_eq!(complete.messages.len(), 1);
        assert!(!complete.truncated);
        assert!(truncated.truncated);
    }

    #[test]
    fn a_listing_without_a_boolean_truncated_flag_breaks_the_contract() {
        let absent = outcome(json!({"messages": []}));
        let text = outcome(json!({"messages": [], "truncated": "true"}));

        for listing in [absent, text] {
            let error = inbox_of(&listing).unwrap_err();
            assert_eq!(error.category, FailureCategory::AutomationContract);
        }
    }

    #[test]
    fn an_absence_stands_only_on_a_complete_list() {
        let complete = Inbox {
            messages: vec![],
            truncated: false,
        };
        let truncated = Inbox {
            messages: vec![],
            truncated: true,
        };

        let proven = complete.prove_absence("o1", "bond-slashed");
        let error = truncated.prove_absence("o1", "bond-slashed").unwrap_err();

        assert!(proven.is_ok());
        assert_eq!(error.category, FailureCategory::Relay);
        assert!(error.message.contains("o1"), "{}", error.message);
        assert!(error.message.contains("bond-slashed"), "{}", error.message);
    }

    #[test]
    fn a_wait_that_ran_out_blames_the_node_only_after_a_complete_read() {
        let waited = Duration::from_secs(1);

        let unread = never_asked("o1", &[PAY_HOLD], waited, None);
        let complete = never_asked("o1", &[PAY_HOLD], waited, Some(false));
        let truncated = never_asked("o1", &[PAY_HOLD], waited, Some(true));

        assert_eq!(unread.category, FailureCategory::Relay);
        assert_eq!(complete.category, FailureCategory::Product);
        assert_eq!(truncated.category, FailureCategory::Relay);
    }
}
