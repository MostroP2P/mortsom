//! Anti-abuse bonds: the bond invoice the node asks for after a take or a
//! new order, walking away from it, and the claim of a slashed bond's share.

use std::time::Duration;

use mortsom_app_contract::{
    AdapterError, BondClaim, DeviceContext, OrderReference, PaymentHandoff, PaymentPath,
};
use mortsom_cli_driver::{Call, unsigned_of};
use mortsom_common::Secret;
use serde_json::Value;

use crate::access::run;
use crate::memo::Memo;
use crate::messages::{
    ADD_BOND_INVOICE, Inbox, MessageWindow, PAY_BOND, list, newest, unix_now, wait_for,
};
use crate::orders;
use crate::payments::require_external;

/// The bond invoice the node asked this actor to pay.
pub(crate) async fn pay(
    device: &DeviceContext<'_>,
    memo: &Memo,
    order: &OrderReference,
    path: PaymentPath,
    window: MessageWindow,
    poll: Duration,
) -> Result<PaymentHandoff, AdapterError> {
    require_external(path)?;
    let invoice = match memo.get(device.actor, &order.id).bond_invoice {
        Some(invoice) => invoice,
        None => wait_for(device, &order.id, &[PAY_BOND], None, window, poll)
            .await?
            .invoice
            .ok_or_else(|| AdapterError::contract("a bond request carries no invoice"))?,
    };
    Ok(PaymentHandoff {
        kind: PaymentPath::ExternalWallet,
        invoice: Some(invoice),
    })
}

/// Walks away from the bond: a taker cancels the take, a maker the order
/// that was never published.
pub(crate) async fn decline(
    device: &DeviceContext<'_>,
    order: &OrderReference,
) -> Result<(), AdapterError> {
    orders::cancel(device, order).await
}

/// The claim of a slashed bond's share, as the node's newest request reads.
pub(crate) async fn claim(
    device: &DeviceContext<'_>,
    memo: &Memo,
    order: &OrderReference,
    window: MessageWindow,
) -> Result<Option<BondClaim>, AdapterError> {
    let inbox = list(device, &order.id, window).await?;
    let submitted_at = memo.get(device.actor, &order.id).claim_submitted_at;
    claim_of(&inbox, &order.id, submitted_at)
}

/// The claim `inbox` shows. No claim, and every phase short of `completed`,
/// rest on an action being absent, which only a complete list read back to
/// the run's start proves (see [`Inbox::prove_absence`]).
fn claim_of(
    inbox: &Inbox,
    order_id: &str,
    submitted_at: Option<i64>,
) -> Result<Option<BondClaim>, AdapterError> {
    let Some(request) = newest(&inbox.messages, &[ADD_BOND_INVOICE], None) else {
        inbox.prove_absence(order_id, "a bond claim request")?;
        return Ok(None);
    };
    let amount_sats = request
        .amount_sats
        .ok_or_else(|| AdapterError::contract("a bond claim request carries no amount"))?;
    let status = claim_status(&inbox.messages, request.created_at, submitted_at);
    let unseen = match status {
        "completed" => None,
        "acknowledged" => Some("bond-payout-completed after the claim request"),
        _ => Some("bond-invoice-accepted or bond-payout-completed after the claim request"),
    };
    if let Some(unseen) = unseen {
        inbox.prove_absence(order_id, unseen)?;
    }
    Ok(Some(BondClaim {
        amount_sats,
        status: status.into(),
    }))
}

/// The claim's phase since the node's newest request, as app v2 names it:
/// `completed` once the share was paid (`bond-payout-completed`),
/// `acknowledged` once the node took the invoice (`bond-invoice-accepted`),
/// `submitted` once this actor handed one in, `pending` before.
fn claim_status(messages: &[Value], requested_at: i64, submitted_at: Option<i64>) -> &'static str {
    let since = |action: &str| {
        messages.iter().any(|message| {
            action_of(message) == Some(action)
                && created_at(message).is_some_and(|at| at >= requested_at)
        })
    };
    if since("bond-payout-completed") {
        "completed"
    } else if since("bond-invoice-accepted") {
        "acknowledged"
    } else if submitted_at.is_some_and(|at| at >= requested_at) {
        "submitted"
    } else {
        "pending"
    }
}

fn action_of(message: &Value) -> Option<&str> {
    message.get("action").and_then(Value::as_str)
}

fn created_at(message: &Value) -> Option<i64> {
    message
        .get("created_at_unix")
        .and_then(unsigned_of)
        .and_then(|at| i64::try_from(at).ok())
}

/// Hands in the claim invoice; returns the claim's phase afterwards.
pub(crate) async fn submit_claim(
    device: &DeviceContext<'_>,
    memo: &Memo,
    order: &OrderReference,
    invoice: &Secret,
    window: MessageWindow,
) -> Result<String, AdapterError> {
    let call = Call::node(&["bond", "add-invoice"])
        .arg("order", order.id.clone())
        .invoice(invoice)
        .confirmed();
    run(device, call).await?;
    let now = i64::try_from(unix_now()).unwrap_or(i64::MAX);
    memo.update(device.actor, &order.id, |r| {
        r.claim_submitted_at = Some(now)
    });
    Ok(match claim(device, memo, order, window).await? {
        Some(claim) => claim.status,
        None => "completed".into(),
    })
}

/// Actions that put an order in a solver's hands.
const DISPUTE_ACTIONS: [&str; 5] = [
    "dispute-initiated-by-you",
    "dispute-initiated-by-peer",
    "admin-took-dispute",
    "admin-settled",
    "admin-canceled",
];

/// The cause of this actor's own slashed bond on `order`: `dispute` when a
/// solver had the order, `timeout` otherwise. The node's `bond-slashed`
/// notice names no cause; a slash is either a ruling or a lapsed deadline.
pub(crate) async fn slash_notice(
    device: &DeviceContext<'_>,
    order: &OrderReference,
    window: MessageWindow,
) -> Result<Option<String>, AdapterError> {
    let inbox = list(device, &order.id, window).await?;
    Ok(proven_slash_cause(&inbox, &order.id)?.map(str::to_owned))
}

fn slash_cause(messages: &[Value]) -> Option<&'static str> {
    if !messages
        .iter()
        .any(|message| action_of(message) == Some("bond-slashed"))
    {
        return None;
    }
    let disputed = messages
        .iter()
        .any(|message| action_of(message).is_some_and(|a| DISPUTE_ACTIONS.contains(&a)));
    Some(if disputed { "dispute" } else { "timeout" })
}

/// The slash cause where `inbox` backs it: `dispute` stands on a present
/// action; no notice (`bond-slashed` absent) and any other cause, `timeout`
/// (every dispute action absent), need a complete list read back to the
/// run's start (see [`Inbox::prove_absence`]).
fn proven_slash_cause(inbox: &Inbox, order_id: &str) -> Result<Option<&'static str>, AdapterError> {
    match slash_cause(&inbox.messages) {
        Some("dispute") => Ok(Some("dispute")),
        None => inbox.prove_absence(order_id, "bond-slashed").map(|()| None),
        timeout => inbox
            .prove_absence(order_id, "a dispute action before the slash")
            .map(|()| timeout),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use mortsom_common::FailureCategory;
    use serde_json::json;

    #[test]
    fn a_claim_is_completed_once_the_share_was_paid() {
        let pending = vec![json!({"action": "add-bond-invoice", "created_at_unix": "10"})];
        let mut accepted = pending.clone();
        accepted.push(json!({"action": "bond-invoice-accepted", "created_at_unix": "11"}));
        let mut paid = accepted.clone();
        paid.push(json!({"action": "bond-payout-completed", "created_at_unix": "12"}));

        assert_eq!(claim_status(&pending, 10, None), "pending");
        assert_eq!(claim_status(&pending, 10, Some(10)), "submitted");
        assert_eq!(claim_status(&pending, 10, Some(9)), "pending");
        assert_eq!(claim_status(&accepted, 10, Some(10)), "acknowledged");
        assert_eq!(claim_status(&paid, 10, Some(10)), "completed");
        assert_eq!(claim_status(&paid, 13, None), "pending");
    }

    #[test]
    fn a_slash_reads_dispute_when_a_solver_had_the_order() {
        let slashed = json!({"action": "bond-slashed", "payload": {"kind": "order"}});
        let disputed = json!({"action": "dispute-initiated-by-peer"});
        let canceled = json!({"action": "canceled"});

        assert_eq!(slash_cause(std::slice::from_ref(&canceled)), None);
        assert_eq!(slash_cause(&[canceled, slashed.clone()]), Some("timeout"));
        assert_eq!(slash_cause(&[disputed, slashed]), Some("dispute"));
    }

    fn inbox(messages: &[Value], truncated: bool, capped: bool) -> Inbox {
        Inbox {
            messages: messages.to_vec(),
            truncated,
            capped,
        }
    }

    /// A list that may be incomplete, and the category of a verdict it
    /// cannot back: truncated (relay), or read through a window short of
    /// the run's start (environment).
    const INCOMPLETE: [(bool, bool, FailureCategory); 2] = [
        (true, false, FailureCategory::Relay),
        (false, true, FailureCategory::EnvironmentMisconfiguration),
    ];

    #[test]
    fn a_claim_on_an_incomplete_list_stands_only_once_paid() {
        let request = json!({"action": "add-bond-invoice", "next_step": "add_bond_invoice",
            "created_at_unix": "10", "payload": {"kind": "bond_payout_request",
            "order": {"sats_amount": "400"}}});
        let accepted = json!({"action": "bond-invoice-accepted", "created_at_unix": "11"});
        let paid = json!({"action": "bond-payout-completed", "created_at_unix": "12"});
        let claim = |messages: &[Value], truncated, capped| {
            claim_of(&inbox(messages, truncated, capped), "o1", Some(10))
                .map(|claim| claim.map(|c| (c.amount_sats, c.status)))
        };

        for (truncated, capped, category) in INCOMPLETE {
            for (messages, unseen) in [
                (vec![], "absence of a bond claim request"),
                (
                    vec![request.clone()],
                    "absence of bond-invoice-accepted or bond-payout-completed after the claim",
                ),
                (
                    vec![request.clone(), accepted.clone()],
                    "absence of bond-payout-completed after the claim",
                ),
            ] {
                let error = claim(&messages, truncated, capped).unwrap_err();
                assert_eq!(error.category, category, "{messages:?}");
                assert!(error.message.contains("o1"), "{}", error.message);
                assert!(error.message.contains(unseen), "{}", error.message);
            }
            let completed = [request.clone(), accepted.clone(), paid.clone()];
            assert_eq!(
                claim(&completed, truncated, capped).unwrap(),
                Some((400, "completed".to_owned()))
            );
        }
        assert_eq!(claim(&[], false, false).unwrap(), None);
        assert_eq!(
            claim(std::slice::from_ref(&request), false, false).unwrap(),
            Some((400, "submitted".to_owned()))
        );
    }

    #[test]
    fn an_incomplete_list_proves_a_slash_for_a_dispute_only() {
        let slashed = json!({"action": "bond-slashed"});
        let disputed = json!({"action": "admin-took-dispute"});
        let canceled = json!({"action": "canceled"});
        let unslashed = [canceled.clone()];
        let lapsed = [canceled, slashed.clone()];
        let ruled = [disputed, slashed];

        for (truncated, capped, category) in INCOMPLETE {
            for messages in [&unslashed[..], &lapsed[..]] {
                let error =
                    proven_slash_cause(&inbox(messages, truncated, capped), "o1").unwrap_err();
                assert_eq!(error.category, category, "{messages:?}");
            }
            let cause = proven_slash_cause(&inbox(&ruled, truncated, capped), "o1").unwrap();
            assert_eq!(cause, Some("dispute"));
        }
        let complete =
            |messages: &[Value]| proven_slash_cause(&inbox(messages, false, false), "o1").unwrap();
        assert_eq!(complete(&unslashed), None);
        assert_eq!(complete(&lapsed), Some("timeout"));
    }
}
