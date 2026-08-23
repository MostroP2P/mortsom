//! Order journeys in the v2 app: create a fixed-price order, discover and
//! take it (amount asked on the take screen), pay the hold invoice through
//! NWC, generate the buyer invoice, report fiat sent, release, cancel and
//! observe the displayed state.

use mortsom_app_contract::{
    AdapterError, AppOrderState, CancelOutcome, DeviceContext, OrderDraft, OrderKind,
    OrderReference, OrderState, PaymentHandoff, PaymentPath, VisibleOrder,
};
use mortsom_common::Secret;

use super::nav::go_home;
use super::{expect, label, present, tap, type_into};
use crate::selectors::Ids;
use crate::states::from_v2_label;

/// Prefix of trade rows in the trades list.
const TRADES_ITEM_PREFIX: &str = "trades.item.";
/// Poll interval while waiting for the new trade row.
const NEW_TRADE_POLL: std::time::Duration = std::time::Duration::from_millis(500);

/// Creates a fixed-price order and returns it with the id shown in the
/// trades list.
pub async fn create(
    device: &DeviceContext<'_>,
    draft: &OrderDraft,
) -> Result<VisibleOrder, AdapterError> {
    let known_before = trade_ids(device).await?;
    go_home(device).await?;
    tap(device, Ids::ORDER_ADD_FAB).await?;
    let side = match draft.kind {
        OrderKind::Sell => Ids::ORDER_ADD_SELL,
        OrderKind::Buy => Ids::ORDER_ADD_BUY,
    };
    tap(device, side).await?;
    tap(device, Ids::ORDER_CREATE_CURRENCY).await?;
    // The picker lists every supported currency in a lazy list, so the row
    // for one further down is never built. Narrowing it is how a user
    // reaches a currency, and how the driver does.
    type_into(device, Ids::ORDER_CREATE_CURRENCY_SEARCH, &draft.fiat_code).await?;
    tap(device, &Ids::currency_option(&draft.fiat_code)).await?;
    type_into(
        device,
        Ids::ORDER_CREATE_FIAT_AMOUNT,
        &draft.fiat_amount.to_string(),
    )
    .await?;
    type_into(
        device,
        Ids::ORDER_CREATE_PAYMENT_METHOD,
        &draft.payment_method,
    )
    .await?;
    if !present(device, Ids::ORDER_CREATE_SATS_AMOUNT).await? {
        tap(device, Ids::ORDER_CREATE_PRICE_TYPE).await?;
    }
    type_into(
        device,
        Ids::ORDER_CREATE_SATS_AMOUNT,
        &draft.amount_sats.to_string(),
    )
    .await?;
    tap(device, Ids::ORDER_CREATE_SUBMIT).await?;
    tap(device, Ids::ORDER_CONFIRM_HOME).await?;
    let id = new_trade_id(device, &known_before).await?;
    let state = observe(device, &OrderReference::with_kind(id.clone(), draft.kind))
        .await?
        .state;
    Ok(VisibleOrder {
        id: Some(id),
        state,
    })
}

/// Opens the book on the side listing `order`, opens it and takes it. The
/// v2 take screen asks the fiat amount for range orders only; a fixed
/// order shows the confirm button directly.
pub async fn find_and_take(
    device: &DeviceContext<'_>,
    order: &OrderReference,
    kind: OrderKind,
) -> Result<(), AdapterError> {
    go_home(device).await?;
    // The tabs are named by their visible label, and each lists the opposite
    // side: the "Buy BTC" tab lists sell orders, because the taker buys.
    // Picking the tab whose name matches the order's kind would open the
    // book that cannot contain it.
    let tab = match kind {
        OrderKind::Sell => Ids::ORDER_BOOK_TAB_BUY,
        OrderKind::Buy => Ids::ORDER_BOOK_TAB_SELL,
    };
    tap(device, tab).await?;
    tap(device, &Ids::order_book_item(&order.id)).await?;
    tap(device, Ids::ORDER_TAKE_CONFIRM).await?;
    expect(device, Ids::ORDER_STATUS).await
}

/// Opens the trade detail of `order` from the trades list.
pub async fn open_trade(
    device: &DeviceContext<'_>,
    order: &OrderReference,
) -> Result<(), AdapterError> {
    go_home(device).await?;
    tap(device, Ids::NAV_TRADES).await?;
    tap(device, &Ids::trades_item(&order.id)).await?;
    expect(device, Ids::ORDER_STATUS).await
}

/// Reads the displayed status; `loading` is reported as an unresolved
/// product state so callers keep waiting rather than mis-mapping it.
pub async fn observe(
    device: &DeviceContext<'_>,
    order: &OrderReference,
) -> Result<AppOrderState, AdapterError> {
    open_trade(device, order).await?;
    let raw = label(device, Ids::ORDER_STATUS).await?;
    let state = from_v2_label(&raw).ok_or_else(|| {
        AdapterError::product(format!(
            "order {} shows unresolved status `{raw}`",
            order.id
        ))
    })?;
    Ok(AppOrderState {
        state,
        raw_label: raw,
    })
}

/// Pays the hold invoice through the connected wallet; returns the invoice
/// shown (protected) for hash correlation.
pub async fn pay_hold_invoice(
    device: &DeviceContext<'_>,
    order: &OrderReference,
) -> Result<PaymentHandoff, AdapterError> {
    open_trade(device, order).await?;
    tap(device, Ids::TRADE_PAY_INVOICE).await?;
    let invoice = label(device, Ids::PAY_INVOICE_TEXT).await?;
    tap(device, Ids::PAY_NWC).await?;
    expect(device, Ids::ORDER_ADD_FAB).await?;
    Ok(PaymentHandoff {
        kind: PaymentPath::Nwc,
        invoice: (!invoice.is_empty()).then(|| Secret::new(invoice)),
    })
}

/// Provides the buyer invoice through the connected wallet.
///
/// v2 asks NWC for the invoice and submits it in one step, so there is
/// nothing to confirm: opening the screen is the whole action. The invoice
/// is read back for payment correlation and is optional — the screen may
/// already have left by the time it is looked for.
///
/// The manual form appearing instead means NWC could not produce an
/// invoice. The harness has no invoice to type, so that is reported rather
/// than worked around.
pub async fn provide_buyer_invoice(
    device: &DeviceContext<'_>,
    order: &OrderReference,
) -> Result<Option<Secret>, AdapterError> {
    open_trade(device, order).await?;
    tap(device, Ids::TRADE_ADD_INVOICE).await?;
    if present(device, Ids::INVOICE_MANUAL).await? || present(device, Ids::INVOICE_TEXT).await? {
        return Err(AdapterError::product(
            "the app fell back to the manual invoice form: the connected wallet did not generate one",
        ));
    }
    let invoice = if present(device, Ids::INVOICE_NWC_TEXT).await? {
        label(device, Ids::INVOICE_NWC_TEXT).await?
    } else {
        String::new()
    };
    Ok((!invoice.is_empty()).then(|| Secret::new(invoice)))
}

/// Reports fiat sent.
pub async fn confirm_fiat_sent(
    device: &DeviceContext<'_>,
    order: &OrderReference,
) -> Result<(), AdapterError> {
    open_trade(device, order).await?;
    tap(device, Ids::TRADE_FIAT_SENT).await
}

/// Releases the sats, confirming the dialog.
pub async fn release(
    device: &DeviceContext<'_>,
    order: &OrderReference,
) -> Result<(), AdapterError> {
    open_trade(device, order).await?;
    tap(device, Ids::TRADE_RELEASE).await?;
    tap(device, Ids::TRADE_RELEASE_CONFIRM).await
}

/// Cancels when the app still offers it; idempotent.
pub async fn cancel_if_open(
    device: &DeviceContext<'_>,
    order: &OrderReference,
) -> Result<CancelOutcome, AdapterError> {
    go_home(device).await?;
    tap(device, Ids::NAV_TRADES).await?;
    if !present(device, &Ids::trades_item(&order.id)).await? {
        return Ok(CancelOutcome::NotFound);
    }
    tap(device, &Ids::trades_item(&order.id)).await?;
    expect(device, Ids::ORDER_STATUS).await?;
    let raw = label(device, Ids::ORDER_STATUS).await?;
    if from_v2_label(&raw).is_some_and(OrderState::is_terminal) {
        return Ok(CancelOutcome::AlreadyClosed);
    }
    if !present(device, Ids::TRADE_CANCEL).await? {
        return Ok(CancelOutcome::AlreadyClosed);
    }
    tap(device, Ids::TRADE_CANCEL).await?;
    tap(device, Ids::TRADE_CANCEL_CONFIRM).await?;
    Ok(CancelOutcome::Cancelled)
}

/// Normalized names of the trade actions currently visible.
pub async fn visible_actions(
    device: &DeviceContext<'_>,
    order: &OrderReference,
) -> Result<Vec<String>, AdapterError> {
    open_trade(device, order).await?;
    let mut actions = Vec::new();
    for (id, name) in [
        (Ids::TRADE_PAY_INVOICE, "pay-invoice"),
        (Ids::TRADE_ADD_INVOICE, "add-invoice"),
        (Ids::TRADE_FIAT_SENT, "fiat-sent"),
        (Ids::TRADE_RELEASE, "release"),
        (Ids::TRADE_CANCEL, "cancel"),
        (Ids::TRADE_DISPUTE, "dispute"),
        (Ids::TRADE_RATE, "rate"),
    ] {
        if present(device, id).await? {
            actions.push(name.to_owned());
        }
    }
    Ok(actions)
}

/// Cancels the order while the seller is being asked to pay the hold
/// invoice.
///
/// The pay screen offers Cancel only on the manual path; with a wallet
/// connected the auto-pay widget covers the screen and the only way out is
/// the trade detail. Both end in the same confirmation dialog, so the
/// outcome is the same either way.
pub async fn cancel_payment(
    device: &DeviceContext<'_>,
    order: &OrderReference,
) -> Result<(), AdapterError> {
    open_trade(device, order).await?;
    tap(device, Ids::TRADE_PAY_INVOICE).await?;
    if present(device, Ids::PAY_CANCEL).await? {
        tap(device, Ids::PAY_CANCEL).await?;
        if present(device, Ids::TRADE_CANCEL_CONFIRM).await? {
            tap(device, Ids::TRADE_CANCEL_CONFIRM).await?;
        }
        return expect(device, Ids::ORDER_ADD_FAB).await;
    }
    open_trade(device, order).await?;
    tap(device, Ids::TRADE_CANCEL).await?;
    tap(device, Ids::TRADE_CANCEL_CONFIRM).await
}

/// Ids of the trades currently listed.
pub async fn trade_ids(device: &DeviceContext<'_>) -> Result<Vec<String>, AdapterError> {
    go_home(device).await?;
    tap(device, Ids::NAV_TRADES).await?;
    let xml = page_source(device).await?;
    Ok(ids_with_prefix(&xml, TRADES_ITEM_PREFIX))
}

async fn new_trade_id(
    device: &DeviceContext<'_>,
    known: &[String],
) -> Result<String, AdapterError> {
    let deadline = tokio::time::Instant::now() + device.ui_timeout;
    let mut now = trade_ids(device).await?;
    loop {
        if let Some(id) = now.iter().find(|id| !known.contains(id)) {
            return Ok(id.clone());
        }
        if tokio::time::Instant::now() >= deadline {
            return Err(AdapterError::product(
                "no new trade appeared in the trades list after creating the order",
            ));
        }
        tokio::time::sleep(NEW_TRADE_POLL).await;
        now = ids_with_prefix(&page_source(device).await?, TRADES_ITEM_PREFIX);
    }
}

async fn page_source(device: &DeviceContext<'_>) -> Result<String, AdapterError> {
    device
        .ui
        .page_source()
        .await
        .map_err(|e| AdapterError::infrastructure(format!("page source: {e}")))
}

/// Extracts `resource-id` values with `prefix` from a UiAutomator XML dump.
pub fn ids_with_prefix(xml: &str, prefix: &str) -> Vec<String> {
    let mut out = Vec::new();
    let needle = format!("resource-id=\"{prefix}");
    let mut rest = xml;
    while let Some(pos) = rest.find(&needle) {
        let after = &rest[pos + needle.len()..];
        if let Some(end) = after.find('"') {
            let id = &after[..end];
            if !out.iter().any(|o| o == id) {
                out.push(id.to_owned());
            }
            rest = &after[end..];
        } else {
            break;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_dynamic_ids_from_hierarchy_xml() {
        let xml = r#"<node resource-id="trades.item.o1"/><node resource-id="trades.item.o2"/><node resource-id="trades.item.o1"/>"#;
        assert_eq!(ids_with_prefix(xml, "trades.item."), vec!["o1", "o2"]);
    }
}
