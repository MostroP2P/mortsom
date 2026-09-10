//! Order journeys: create a fixed-price order, discover and take it, pay the
//! hold invoice, provide the buyer invoice, report fiat sent, release,
//! cancel and observe the displayed state.

use mortsom_app_contract::{
    AdapterError, AppOrderState, CancelOutcome, DeviceContext, FiatAmount, OrderDraft, OrderKind,
    OrderPrice, OrderReference, OrderState, PaymentHandoff, PaymentPath, VisibleOrder,
};
use mortsom_common::Secret;

use super::{expect, label, listed, present, tap, type_into};
use crate::selectors::Ids;

/// Prefix of trade rows in the trades list.
const TRADES_ITEM_PREFIX: &str = "trades.item.";
/// Deepest navigation stack `go_home` unwinds (trade detail over trades
/// list over the book, plus one dialog).
const MAX_BACK_STEPS: usize = 4;

/// Creates a fixed-price order and returns it with the id the app shows in
/// the trades list.
pub async fn create(
    device: &DeviceContext<'_>,
    draft: &OrderDraft,
) -> Result<VisibleOrder, AdapterError> {
    // The classic app is frozen (spec §6.3): only the ordinary fixed-price
    // order it always created is driven.
    let (OrderPrice::Fixed { amount_sats }, FiatAmount::Exact(fiat_amount)) =
        (draft.price, draft.fiat)
    else {
        return Err(AdapterError::contract(
            "the classic app adapter creates fixed-price orders for one fiat amount only",
        ));
    };
    let known_before = trade_ids(device).await?;
    go_home(device).await?;
    tap(device, Ids::ORDER_ADD_FAB).await?;
    let side = match draft.kind {
        OrderKind::Sell => Ids::ORDER_ADD_SELL,
        OrderKind::Buy => Ids::ORDER_ADD_BUY,
    };
    tap(device, side).await?;
    tap(device, Ids::ORDER_CREATE_CURRENCY).await?;
    tap(device, &Ids::currency_option(&draft.fiat_code)).await?;
    type_into(
        device,
        Ids::ORDER_CREATE_FIAT_AMOUNT,
        &fiat_amount.to_string(),
    )
    .await?;
    type_into(
        device,
        Ids::ORDER_CREATE_PAYMENT_METHOD,
        &draft.payment_method,
    )
    .await?;
    // The price type toggle starts on market rate; switching to fixed
    // reveals the sats amount field. Whether that field is already there is
    // a question about the whole form, not about the part of it the
    // keyboard has not covered: answered by looking only at what is
    // visible, a form already on fixed price gets toggled onto market, its
    // sats field stops taking input, and submit stays disabled for the rest
    // of the scenario.
    if !listed(device, Ids::ORDER_CREATE_SATS_AMOUNT).await? {
        tap(device, Ids::ORDER_CREATE_PRICE_TYPE).await?;
    }
    type_into(
        device,
        Ids::ORDER_CREATE_SATS_AMOUNT,
        &amount_sats.to_string(),
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

/// Opens the order book on the side that lists `order` and takes it.
pub async fn find_and_take(
    device: &DeviceContext<'_>,
    order: &OrderReference,
    kind: OrderKind,
) -> Result<(), AdapterError> {
    go_home(device).await?;
    // The tabs are named by their visible label, and each lists the opposite
    // side: the "Buy BTC" tab lists sell orders, because the taker buys
    // (`AutomationIds.orderBookTabBuy` sits on the tab that filters for
    // `OrderType.sell`). Picking the tab whose name matches the order's kind
    // would open the book that cannot contain it.
    let tab = match kind {
        OrderKind::Sell => Ids::ORDER_BOOK_TAB_BUY,
        OrderKind::Buy => Ids::ORDER_BOOK_TAB_SELL,
    };
    tap(device, tab).await?;
    tap(device, &Ids::order_book_item(&order.id)).await?;
    tap(device, Ids::ORDER_TAKE_CONFIRM).await?;
    // Taking lands on the trade detail of the new session.
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

/// Reads the displayed status of `order`.
pub async fn observe(
    device: &DeviceContext<'_>,
    order: &OrderReference,
) -> Result<AppOrderState, AdapterError> {
    open_trade(device, order).await?;
    let raw = label(device, Ids::ORDER_STATUS).await?;
    let state = OrderState::from_protocol_status(&raw).ok_or_else(|| {
        AdapterError::product(format!("order {} shows unknown status `{raw}`", order.id))
    })?;
    Ok(AppOrderState {
        state,
        raw_label: raw,
    })
}

/// Pays the hold invoice through the connected wallet and returns the
/// invoice the app displayed (protected) for hash correlation.
pub async fn pay_hold_invoice(
    device: &DeviceContext<'_>,
    order: &OrderReference,
    path: PaymentPath,
) -> Result<PaymentHandoff, AdapterError> {
    open_trade(device, order).await?;
    tap(device, Ids::TRADE_PAY_INVOICE).await?;
    // The label is rendered for humans; padding is not part of the invoice
    // and a whitespace-only label carries none at all.
    let invoice = label(device, Ids::PAY_INVOICE_TEXT)
        .await?
        .trim()
        .to_owned();
    if path == PaymentPath::ExternalWallet {
        // Nothing is tapped: the app has no wallet to pay with, and the
        // caller holds the node that does. The screen stays open until the
        // daemon sees the payment, which is the app's own signal to move
        // on — pressing anything here would be inventing one.
        if invoice.is_empty() {
            return Err(AdapterError::product(
                "the pay screen shows no invoice, so there is nothing to pay externally",
            ));
        }
        return Ok(PaymentHandoff {
            kind: PaymentPath::ExternalWallet,
            invoice: Some(Secret::new(invoice)),
        });
    }
    tap(device, Ids::PAY_NWC).await?;
    // On success the app returns home; the wallet observer confirms the
    // settlement, the adapter only reports the handoff.
    expect(device, Ids::ORDER_ADD_FAB).await?;
    Ok(PaymentHandoff {
        kind: PaymentPath::Nwc,
        invoice: (!invoice.is_empty()).then(|| Secret::new(invoice)),
    })
}

/// Generates and submits the buyer invoice through the connected wallet.
/// Returns the invoice (protected) for hash correlation. After confirming,
/// the app returns to the order book; the trade detail is reached again
/// only once the daemon answers, so callers observe the order afterwards.
pub async fn provide_buyer_invoice(
    device: &DeviceContext<'_>,
    order: &OrderReference,
    supplied: Option<&Secret>,
) -> Result<Option<Secret>, AdapterError> {
    open_trade(device, order).await?;
    tap(device, Ids::TRADE_ADD_INVOICE).await?;
    if let Some(invoice) = supplied {
        // The caller issued this one on a node it holds; the app only has
        // to accept it, through the same form a user types into.
        type_into(device, Ids::INVOICE_TEXT, invoice.expose()).await?;
        tap(device, Ids::INVOICE_SUBMIT).await?;
        expect(device, Ids::ORDER_ADD_FAB).await?;
        return Ok(Some(invoice.clone()));
    }
    tap(device, Ids::INVOICE_NWC_GENERATE).await?;
    let invoice = label(device, Ids::INVOICE_NWC_TEXT)
        .await?
        .trim()
        .to_owned();
    tap(device, Ids::INVOICE_NWC_CONFIRM).await?;
    expect(device, Ids::ORDER_ADD_FAB).await?;
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

/// Cancels the order when the app still offers cancellation. Idempotent:
/// a terminal or unknown order is reported, never re-cancelled.
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
    if OrderState::from_protocol_status(&raw).is_some_and(OrderState::is_terminal) {
        return Ok(CancelOutcome::AlreadyClosed);
    }
    if !present(device, Ids::TRADE_CANCEL).await? {
        return Ok(CancelOutcome::AlreadyClosed);
    }
    tap(device, Ids::TRADE_CANCEL).await?;
    tap(device, Ids::TRADE_CANCEL_CONFIRM).await?;
    Ok(CancelOutcome::Cancelled)
}

/// Normalized names of the trade actions currently visible for `order`.
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
        (Ids::trade_action("rate").as_str(), "rate"),
    ] {
        if present(device, id).await? {
            actions.push(name.to_owned());
        }
    }
    Ok(actions)
}

/// Opens the pay screen and cancels the order from there.
pub async fn cancel_payment(
    device: &DeviceContext<'_>,
    order: &OrderReference,
) -> Result<(), AdapterError> {
    open_trade(device, order).await?;
    tap(device, Ids::TRADE_PAY_INVOICE).await?;
    tap(device, Ids::PAY_CANCEL).await?;
    expect(device, Ids::ORDER_ADD_FAB).await
}

/// Returns to the order book from wherever the app is (drawer screens and
/// trade detail have a back control; the book itself has the FAB).
pub async fn go_home(device: &DeviceContext<'_>) -> Result<(), AdapterError> {
    for _ in 0..MAX_BACK_STEPS {
        if present(device, Ids::ORDER_ADD_FAB).await? {
            return Ok(());
        }
        if present(device, Ids::APPBAR_BACK).await? {
            tap(device, Ids::APPBAR_BACK).await?;
        } else if present(device, Ids::NAV_ORDER_BOOK).await? {
            tap(device, Ids::NAV_ORDER_BOOK).await?;
        } else {
            break;
        }
    }
    expect(device, Ids::ORDER_ADD_FAB).await.map_err(|e| {
        AdapterError::contract(format!(
            "could not return to the order book within {MAX_BACK_STEPS} back steps: {e}"
        ))
    })
}

/// Ids of the trades currently listed, read from the accessibility tree.
pub async fn trade_ids(device: &DeviceContext<'_>) -> Result<Vec<String>, AdapterError> {
    go_home(device).await?;
    tap(device, Ids::NAV_TRADES).await?;
    let xml = device
        .ui
        .page_source()
        .await
        .map_err(|e| AdapterError::infrastructure(format!("page source: {e}")))?;
    Ok(ids_with_prefix(&xml, TRADES_ITEM_PREFIX))
}

/// The trade id that appeared since `known` was captured. Navigates to the
/// trades list once, then polls the hierarchy without re-navigating.
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
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        let xml = device
            .ui
            .page_source()
            .await
            .map_err(|e| AdapterError::infrastructure(format!("page source: {e}")))?;
        now = ids_with_prefix(&xml, TRADES_ITEM_PREFIX);
    }
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
        let xml = r#"<node resource-id="trades.item.o1"/><node resource-id="trades.item.o2"/><node resource-id="trades.item.o1"/><node resource-id="other.x"/>"#;
        assert_eq!(ids_with_prefix(xml, "trades.item."), vec!["o1", "o2"]);
        assert!(ids_with_prefix("<hierarchy/>", "trades.item.").is_empty());
    }
}
