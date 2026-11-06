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
use crate::messages::{ADD_BOND_INVOICE, PAY_BOND, list, newest, wait_for};
use crate::orders;
use crate::payments::require_external;

/// The bond invoice the node asked this actor to pay.
pub(crate) async fn pay(
    device: &DeviceContext<'_>,
    memo: &Memo,
    order: &OrderReference,
    path: PaymentPath,
    poll: Duration,
) -> Result<PaymentHandoff, AdapterError> {
    require_external(path)?;
    let invoice = match memo.get(device.actor, &order.id).bond_invoice {
        Some(invoice) => invoice,
        None => wait_for(device, &order.id, &[PAY_BOND], None, poll)
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
) -> Result<Option<BondClaim>, AdapterError> {
    let messages = list(device, &order.id).await?;
    let Some(request) = newest(&messages, &[ADD_BOND_INVOICE], None) else {
        return Ok(None);
    };
    let amount_sats = request
        .amount_sats
        .ok_or_else(|| AdapterError::contract("a bond claim request carries no amount"))?;
    Ok(Some(BondClaim {
        amount_sats,
        status: claim_status(
            &messages,
            request.created_at,
            memo.get(device.actor, &order.id).claim_submitted_at,
        )
        .into(),
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
) -> Result<String, AdapterError> {
    let call = Call::node(&["bond", "add-invoice"])
        .arg("order", order.id.clone())
        .invoice(invoice)
        .confirmed();
    run(device, call).await?;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX))
        .unwrap_or(0);
    memo.update(device.actor, &order.id, |r| {
        r.claim_submitted_at = Some(now)
    });
    Ok(match claim(device, memo, order).await? {
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
) -> Result<Option<String>, AdapterError> {
    Ok(slash_cause(&list(device, &order.id).await?).map(str::to_owned))
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

#[cfg(test)]
mod tests {
    use super::*;
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
}
