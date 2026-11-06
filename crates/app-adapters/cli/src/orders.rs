//! Orders: creating one (`order new`), taking one (`order take-sell` /
//! `order take-buy`), reading its state (`order info`), listing the actor's
//! own (`order mine`) and cancelling (`order cancel`).

use mortsom_app_contract::{
    AdapterError, AppOrderState, CancelOutcome, DeviceContext, FiatAmount, OrderDraft, OrderKind,
    OrderPrice, OrderReference, OrderState, VisibleOrder,
};
use mortsom_cli_driver::{Call, CliError, Envelope};
use mortsom_common::Secret;
use serde_json::Value;

use crate::access::{adapter_error, missing, run, try_run};
use crate::memo::Memo;
use crate::messages::{ADD_INVOICE, PAY_BOND, PAY_HOLD};

/// Node reasons for a cancel refused because the order is already closed.
const CLOSED_REASONS: [&str; 3] = [
    "order_already_canceled",
    "is_not_your_order",
    "not_allowed_by_status",
];

fn side(kind: OrderKind) -> &'static str {
    match kind {
        OrderKind::Sell => "sell",
        OrderKind::Buy => "buy",
    }
}

/// `order new` for `draft`, asking for `expiry_secs` when set.
pub(crate) fn new_order_call(draft: &OrderDraft, expiry_secs: Option<u64>) -> Call {
    let mut call = Call::node(&["order", "new"])
        .arg("side", side(draft.kind))
        .arg("currency", draft.fiat_code.clone())
        .arg("payment-method", draft.payment_method.clone());
    call = match draft.fiat {
        FiatAmount::Exact(amount) => call.arg("fiat-amount", amount.to_string()),
        FiatAmount::Range { min, max } => call
            .arg("fiat-amount", min.to_string())
            .arg("fiat-amount-max", max.to_string()),
    };
    call = match draft.price {
        OrderPrice::Fixed { amount_sats } => call.arg("sats", amount_sats.to_string()),
        OrderPrice::Market { premium } => call.arg("sats", "0").arg("premium", premium.to_string()),
    };
    call.arg_opt("expires-in", expiry_secs.map(|secs| secs.to_string()))
        .confirmed()
}

/// Creates the order and remembers a bond the node asked for.
pub(crate) async fn create(
    device: &DeviceContext<'_>,
    memo: &Memo,
    draft: &OrderDraft,
    expiry_secs: Option<u64>,
) -> Result<VisibleOrder, AdapterError> {
    let outcome = run(device, new_order_call(draft, expiry_secs)).await?;
    let id = outcome
        .text(&["order", "id"])
        .ok_or_else(|| missing("order.new", "order.id"))?
        .to_owned();
    let status = outcome
        .text(&["order", "status"])
        .ok_or_else(|| missing("order.new", "order.status"))?;
    let state = state_of("order.new", status)?;
    remember_request(memo, device, &id, &outcome);
    Ok(VisibleOrder {
        id: Some(id),
        state,
    })
}

/// Takes `order`, at `fiat_amount` for a range order.
pub(crate) async fn take(
    device: &DeviceContext<'_>,
    memo: &Memo,
    order: &OrderReference,
    fiat_amount: Option<u64>,
) -> Result<(), AdapterError> {
    let kind = match order.kind {
        Some(kind) => kind,
        None => side_on_book(device, &order.id).await?,
    };
    let words: [&str; 2] = match kind {
        OrderKind::Sell => ["order", "take-sell"],
        OrderKind::Buy => ["order", "take-buy"],
    };
    let call = Call::node(&words)
        .arg("order", order.id.clone())
        .arg_opt("amount", fiat_amount.map(|amount| amount.to_string()))
        .confirmed();
    let outcome = run(device, call).await?;
    remember_request(memo, device, &order.id, &outcome);
    Ok(())
}

/// Keeps what a reply asked for: the invoice to pay or the payout amount.
fn remember_request(memo: &Memo, device: &DeviceContext<'_>, order_id: &str, outcome: &Envelope) {
    let invoice = outcome.text(&["payment_request"]).map(Secret::new);
    let payout = outcome
        .unsigned(&["order", "sats_amount"])
        .filter(|s| *s > 0);
    match outcome.text(&["next_step"]) {
        Some(PAY_HOLD) => memo.update(device.actor, order_id, |r| r.hold_invoice = invoice),
        Some(ADD_INVOICE) => memo.update(device.actor, order_id, |r| r.payout_sats = payout),
        _ => {}
    }
}

/// The side of an order still on the public book.
async fn side_on_book(
    device: &DeviceContext<'_>,
    order_id: &str,
) -> Result<OrderKind, AdapterError> {
    let outcome = run(device, Call::node(&["order", "list"])).await?;
    let orders = outcome
        .field(&["orders"])
        .and_then(Value::as_array)
        .ok_or_else(|| missing("order.list", "orders"))?;
    let side = orders
        .iter()
        .find(|o| o.get("id").and_then(Value::as_str) == Some(order_id))
        .and_then(|o| o.get("side").and_then(Value::as_str))
        .ok_or_else(|| AdapterError::product(format!("order {order_id} is not on the book")))?;
    match side {
        "sell" => Ok(OrderKind::Sell),
        "buy" => Ok(OrderKind::Buy),
        other => Err(AdapterError::contract(format!(
            "unknown order side `{other}`"
        ))),
    }
}

/// The order's state as the node reports it to its party.
pub(crate) async fn observe(
    device: &DeviceContext<'_>,
    order: &OrderReference,
) -> Result<AppOrderState, AdapterError> {
    let call = Call::node(&["order", "info"]).arg("order", order.id.clone());
    let outcome = run(device, call).await?;
    let orders = outcome
        .field(&["orders"])
        .and_then(Value::as_array)
        .ok_or_else(|| missing("order.info", "orders"))?;
    let reported = orders
        .iter()
        .find(|o| o.get("id").and_then(Value::as_str) == Some(order.id.as_str()))
        .and_then(|o| o.get("status").and_then(Value::as_str));
    let Some(status) = reported else {
        if parked_on_bond(device, &order.id).await? {
            return Ok(AppOrderState {
                state: OrderState::Taken,
                raw_label: "waiting-bond".into(),
            });
        }
        return Err(AdapterError::product(format!(
            "the node does not report order {} to this user",
            order.id
        )));
    };
    // The node parks a take on its bond for that taker alone, and reports
    // the order only to the maker meanwhile: for the maker it is still
    // pending, as the book and app v2 show it.
    let status = if status == "waiting-taker-bond" {
        "pending"
    } else {
        status
    };
    let state = state_of("order.info", status)?;
    let raw_label = match state {
        OrderState::Completed => rating_label(device, order, status).await?,
        OrderState::PayoutPending => payout_label(device, order, status).await?,
        _ => status.to_owned(),
    };
    Ok(AppOrderState { state, raw_label })
}

/// After a failed payout the order stays `settled-hold-invoice` while the
/// node asks the buyer for a new invoice; app v2 shows that request as
/// `waiting-invoice`, and so does the CLI's newest message.
async fn payout_label(
    device: &DeviceContext<'_>,
    order: &OrderReference,
    status: &str,
) -> Result<String, AdapterError> {
    let messages = crate::messages::list(device, &order.id).await?;
    let asked = messages
        .last()
        .and_then(|m| m.get("next_step"))
        .and_then(Value::as_str)
        == Some(ADD_INVOICE);
    Ok(if asked {
        "waiting-invoice".into()
    } else {
        status.to_owned()
    })
}

/// What a completed trade shows about this user's rating, as app v2 labels
/// it: `rated` once the node confirmed this user's rating, `pending-rating`
/// while its invitation to rate stands, the status otherwise.
async fn rating_label(
    device: &DeviceContext<'_>,
    order: &OrderReference,
    status: &str,
) -> Result<String, AdapterError> {
    let messages = crate::messages::list(device, &order.id).await?;
    let has = |action: &str| {
        messages
            .iter()
            .any(|m| m.get("action").and_then(Value::as_str) == Some(action))
    };
    Ok(if has("rate-received") {
        "rated".into()
    } else if has("rate") {
        "pending-rating".into()
    } else {
        status.to_owned()
    })
}

fn state_of(command: &str, status: &str) -> Result<OrderState, AdapterError> {
    OrderState::from_protocol_status(status).ok_or_else(|| {
        AdapterError::contract(format!("`{command}` reports unknown status `{status}`"))
    })
}

/// The ids of the orders in the actor's own store.
pub(crate) async fn owned(
    device: &DeviceContext<'_>,
    sync: bool,
) -> Result<Vec<String>, AdapterError> {
    if sync {
        // The node announces a range order's remainder to a fresh trade
        // key; `order sync` gives it a local row, as the app's restore does.
        run(device, Call::node(&["order", "sync"])).await?;
    }
    let outcome = run(device, Call::local(&["order", "mine"])).await?;
    let orders = outcome
        .field(&["orders"])
        .and_then(Value::as_array)
        .ok_or_else(|| missing("order.mine", "orders"))?;
    let local: Vec<String> = orders
        .iter()
        .map(|o| {
            o.get("id")
                .and_then(Value::as_str)
                .map(str::to_owned)
                .ok_or_else(|| missing("order.mine", "orders[].id"))
        })
        .collect::<Result<_, _>>()?;
    still_reported(device, local).await
}

/// The local rows the node still reports to this user. A store keeps every
/// row it ever wrote; an order whose take was undone, or that the node no
/// longer counts as this user's, is not theirs any more -- as app v2 stops
/// listing it.
async fn still_reported(
    device: &DeviceContext<'_>,
    local: Vec<String>,
) -> Result<Vec<String>, AdapterError> {
    if local.is_empty() {
        return Ok(local);
    }
    let call = local
        .iter()
        .fold(Call::node(&["order", "info"]), |call, id| {
            call.arg("order", id.clone())
        });
    let outcome = run(device, call).await?;
    let reported: Vec<&str> = outcome
        .field(&["orders"])
        .and_then(Value::as_array)
        .ok_or_else(|| missing("order.info", "orders"))?
        .iter()
        .filter_map(|o| o.get("id").and_then(Value::as_str))
        .collect();
    let mut owned = Vec::with_capacity(local.len());
    for id in local {
        if reported.contains(&id.as_str()) || parked_on_bond(device, &id).await? {
            owned.push(id);
        }
    }
    Ok(owned)
}

/// A take parked on its bond: the node counts the taker as a party only
/// once the bond locks, so until then the actor's own newest message, the
/// bond request, is the trade -- held while its invoice has not expired,
/// as app v2 holds the pay-bond screen.
async fn parked_on_bond(device: &DeviceContext<'_>, order_id: &str) -> Result<bool, AdapterError> {
    let messages = crate::messages::list(device, order_id).await?;
    Ok(messages.last().is_some_and(|newest| {
        newest.get("next_step").and_then(Value::as_str) == Some(PAY_BOND) && bond_live(newest)
    }))
}

/// Whether the bond invoice a request carries is still payable.
fn bond_live(request: &Value) -> bool {
    request
        .pointer("/payload/invoice")
        .and_then(Value::as_str)
        .and_then(|text| text.parse::<lightning_invoice::Bolt11Invoice>().ok())
        .is_none_or(|invoice| {
            let expires = invoice.duration_since_epoch() + invoice.expiry_time();
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .is_ok_and(|now| now < expires)
        })
}

/// Cancels the order unless the node already closed it.
pub(crate) async fn cancel_if_open(
    device: &DeviceContext<'_>,
    order: &OrderReference,
) -> Result<CancelOutcome, AdapterError> {
    if let Ok(state) = observe(device, order).await
        && state.state.is_terminal()
    {
        return Ok(CancelOutcome::AlreadyClosed);
    }
    let call = Call::node(&["order", "cancel"])
        .arg("order", order.id.clone())
        .confirmed();
    match try_run(device, call).await? {
        Ok(_) => Ok(CancelOutcome::Cancelled),
        Err(error) if error.code() == Some("order_unknown") => Ok(CancelOutcome::NotFound),
        Err(error) if is_closed_refusal(&error) => closed_after_refusal(device, order, error).await,
        Err(error) if error.reason() == Some("invalid_pubkey") => {
            already_requested(device, order, error).await
        }
        Err(error) => Err(adapter_error(error)),
    }
}

/// The node refuses a cooperative cancel from the side that already asked
/// for it (`invalid_pubkey`): this side's request stands and only the
/// counterpart can complete it, as the app's own cancel shows.
async fn already_requested(
    device: &DeviceContext<'_>,
    order: &OrderReference,
    error: CliError,
) -> Result<CancelOutcome, AdapterError> {
    let messages = crate::messages::list(device, &order.id).await?;
    let requested = messages.iter().any(|message| {
        message.get("action").and_then(Value::as_str) == Some("cooperative-cancel-initiated-by-you")
    });
    if requested {
        Ok(CancelOutcome::Cancelled)
    } else {
        Err(adapter_error(error))
    }
}

fn is_closed_refusal(error: &CliError) -> bool {
    error.code() == Some("node_refused")
        && error
            .reason()
            .is_some_and(|reason| CLOSED_REASONS.contains(&reason))
}

/// A refusal proves nothing by itself: the order must read closed.
async fn closed_after_refusal(
    device: &DeviceContext<'_>,
    order: &OrderReference,
    error: CliError,
) -> Result<CancelOutcome, AdapterError> {
    match observe(device, order).await {
        Ok(state) if state.state.is_terminal() => Ok(CancelOutcome::AlreadyClosed),
        _ => Err(adapter_error(error)),
    }
}

/// Cancels instead of paying the hold invoice.
pub(crate) async fn cancel(
    device: &DeviceContext<'_>,
    order: &OrderReference,
) -> Result<(), AdapterError> {
    let call = Call::node(&["order", "cancel"])
        .arg("order", order.id.clone())
        .confirmed();
    run(device, call).await.map(|_| ())
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use mortsom_cli_driver::NodeTarget;

    fn argv(call: &Call) -> Vec<String> {
        call.argv(&NodeTarget {
            mostro_pubkey: "pk".into(),
            relays: vec!["ws://r".into()],
            reply_timeout_secs: None,
        })
    }

    fn has(argv: &[String], flag: &str, value: &str) -> bool {
        argv.windows(2).any(|w| w[0] == flag && w[1] == value)
    }

    #[test]
    fn a_fixed_order_names_its_sats_and_a_short_lifetime() {
        let draft = OrderDraft::fixed(OrderKind::Sell, 10_000, "USD", 5, "cash");

        let argv = argv(&new_order_call(&draft, Some(120)));

        assert!(has(&argv, "--side", "sell"));
        assert!(has(&argv, "--sats", "10000"));
        assert!(has(&argv, "--fiat-amount", "5"));
        assert!(has(&argv, "--expires-in", "120"));
        assert!(argv.contains(&"--yes".to_owned()));
    }

    #[test]
    fn a_range_order_is_market_priced_with_both_bounds() {
        let draft = OrderDraft::range(OrderKind::Buy, "USD", 5, 15, "cash");

        let argv = argv(&new_order_call(&draft, None));

        assert!(has(&argv, "--side", "buy"));
        assert!(has(&argv, "--fiat-amount", "5"));
        assert!(has(&argv, "--fiat-amount-max", "15"));
        assert!(has(&argv, "--sats", "0"));
        assert!(has(&argv, "--premium", "0"));
        assert!(!argv.contains(&"--expires-in".to_owned()));
    }
}
