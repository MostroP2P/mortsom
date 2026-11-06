//! What the node asked an actor to do next on an order, read with
//! `message list`: the requests that arrive while the actor is idle (a hold
//! invoice for the maker, a payout request after the seller paid, a bond, a
//! slashed bond's share).

use std::time::Duration;

use mortsom_app_contract::{AdapterError, DeviceContext};
use mortsom_cli_driver::{Call, unsigned_of};
use mortsom_common::Secret;
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

/// The messages the node sent this actor about `order_id`, oldest first.
pub(crate) async fn list(
    device: &DeviceContext<'_>,
    order_id: &str,
) -> Result<Vec<Value>, AdapterError> {
    let call = Call::node(&["message", "list"])
        .arg("order", order_id)
        .arg("since", HISTORY_MINUTES);
    let outcome = run(device, call).await?;
    outcome
        .field(&["messages"])
        .and_then(Value::as_array)
        .cloned()
        .ok_or_else(|| missing("message.list", "messages"))
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
/// than `after`.
pub(crate) async fn wait_for(
    device: &DeviceContext<'_>,
    order_id: &str,
    steps: &[&str],
    after: Option<i64>,
    poll: Duration,
) -> Result<Request, AdapterError> {
    let deadline = Instant::now() + device.ui_timeout;
    let waited = tokio::time::timeout_at(deadline, async {
        loop {
            if let Some(request) = newest(&list(device, order_id).await?, steps, after) {
                return Ok(request);
            }
            tokio::time::sleep(poll).await;
        }
    })
    .await;
    // A read cut short by the deadline is abandoned whole: the CLI driver
    // reaps its child and clears its runtime marker on its own.
    waited.unwrap_or_else(|_| {
        Err(AdapterError::product(format!(
            "the node did not ask for {} on order {order_id} within {:?}",
            steps.join(" or "),
            device.ui_timeout
        )))
    })
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
}
