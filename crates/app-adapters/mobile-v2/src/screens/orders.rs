//! Order journeys in the v2 app: create an order (fixed, market or range
//! priced), discover and take it (amount asked on the take screen for a
//! range), pay the hold invoice through NWC, generate the buyer invoice,
//! report fiat sent, release, dispute, rate, cancel and observe the
//! displayed state.

use mortsom_app_contract::{
    AdapterError, AppOrderState, CancelOutcome, DeviceContext, FiatAmount, OrderDraft, OrderKind,
    OrderPrice, OrderReference, OrderState, PaymentHandoff, PaymentPath, VisibleOrder,
};
use mortsom_common::Secret;

use super::nav::go_home;
use super::trades_list::trade_ids;
use super::{SCREEN_POLL, dismiss_keyboard, expect, label, listed, present, tap, type_into};
use crate::selectors::Ids;
use crate::states::from_v2_label;

/// Prefix of trade rows in the trades list.
pub(super) const TRADES_ITEM_PREFIX: &str = "trades.item.";
/// Poll interval while waiting for the new trade row.
const NEW_TRADE_POLL: std::time::Duration = std::time::Duration::from_millis(500);

/// Highest star on the rating screen.
const MAX_RATING: u8 = 5;

/// Creates an order and returns it with the id shown in the trades list.
pub async fn create(
    device: &DeviceContext<'_>,
    draft: &OrderDraft,
) -> Result<VisibleOrder, AdapterError> {
    let known_before = trade_ids(device).await?;
    fill_create_form(device, draft).await?;
    // A form that shows every value yet keeps submit disabled has lost an
    // edit (the second order of one session, 2026-09-10, showed the amount,
    // the method and the price with submit greyed out, twice). Typing is
    // not a business action, so the form is left and filled once more on a
    // fresh screen; one still disabled after that is the product's answer.
    if !submit_ready(device).await? {
        tap(device, Ids::ORDER_CREATE_CANCEL).await?;
        fill_create_form(device, draft).await?;
        if !submit_ready(device).await? {
            return Err(AdapterError::product(
                "the create form kept submit disabled with every field filled, even on a fresh form filled again",
            ));
        }
    }
    // A still-enabled button does not prove the daemon rejected the first
    // request. Submit once, then resolve its outcome through public UI.
    tap(device, Ids::ORDER_CREATE_SUBMIT).await?;
    if super::appears_within(device, Ids::ORDER_CONFIRM_HOME, device.ui_timeout).await? {
        // Closing the new order's screen pops back to the book, which is
        // drawn a moment after the screen leaves. Until then that screen's
        // back is still up, and a look for the book would press it just as
        // it goes: wait for the book the close returns to.
        super::tap_unless_taken(
            device,
            Ids::ORDER_CONFIRM_HOME,
            Ids::ORDER_ADD_FAB,
            super::TAP_PATIENCE,
        )
        .await?;
    }
    let id = new_trade_id(device, &known_before).await?;
    let state = observe(device, &OrderReference::with_kind(id.clone(), draft.kind))
        .await?
        .state;
    Ok(VisibleOrder {
        id: Some(id),
        state,
    })
}

/// Opens the create form from the book and fills every field of `draft`,
/// leaving the keyboard down and submit untouched.
async fn fill_create_form(
    device: &DeviceContext<'_>,
    draft: &OrderDraft,
) -> Result<(), AdapterError> {
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
    super::choose(
        device,
        &Ids::currency_option(&draft.fiat_code),
        Ids::ORDER_CREATE_CURRENCY_SEARCH,
    )
    .await?;
    enter_fiat_amount(device, draft.fiat).await?;
    choose_payment_method(device, &draft.payment_method).await?;
    enter_price(device, draft).await?;
    // The last field is numeric, so the keyboard is still up over the
    // bottom of the form -- submit included.
    dismiss_keyboard(device).await;
    Ok(())
}

/// Whether the create form's submit accepts a press, within a bounded
/// settle: validation runs on the last keystroke and the keyboard has just
/// gone down, so one disabled read is not yet an answer.
async fn submit_ready(device: &DeviceContext<'_>) -> Result<bool, AdapterError> {
    let deadline = tokio::time::Instant::now() + ID_SETTLE;
    loop {
        if device
            .ui
            .is_enabled(Ids::ORDER_CREATE_SUBMIT)
            .await
            .map_err(|e| {
                AdapterError::infrastructure(format!("probe `order.create.submit`: {e}"))
            })?
        {
            return Ok(true);
        }
        if tokio::time::Instant::now() >= deadline {
            return Ok(false);
        }
        tokio::time::sleep(SCREEN_POLL).await;
    }
}

/// Chooses the method on the payment-method screen. Any method, listed for
/// the currency or not, goes through the free-text sheet: the app selects
/// the listed entry when the name matches one, and confirm is the only way
/// the choice reaches the form.
async fn choose_payment_method(
    device: &DeviceContext<'_>,
    method: &str,
) -> Result<(), AdapterError> {
    tap(device, Ids::ORDER_CREATE_PAYMENT_METHOD_ADD).await?;
    tap(device, Ids::ORDER_CREATE_PAYMENT_METHOD_CUSTOM_OPEN).await?;
    type_into(device, Ids::ORDER_CREATE_PAYMENT_METHOD, method).await?;
    tap(device, Ids::ORDER_CREATE_PAYMENT_METHOD_CUSTOM_ADD).await?;
    tap(device, Ids::ORDER_CREATE_PAYMENT_METHODS_CONFIRM).await?;
    // Back on the form: `Add` lives there only, clear of the keyboard that
    // may still cover submit.
    expect(device, Ids::ORDER_CREATE_PAYMENT_METHOD_ADD).await
}

/// Fills the fiat side of the form: one amount, or the range segment and
/// its two bounds.
async fn enter_fiat_amount(
    device: &DeviceContext<'_>,
    fiat: FiatAmount,
) -> Result<(), AdapterError> {
    match fiat {
        FiatAmount::Exact(amount) => {
            type_into(device, Ids::ORDER_CREATE_FIAT_AMOUNT, &amount.to_string()).await
        }
        FiatAmount::Range { min, max } => {
            tap(device, Ids::ORDER_CREATE_AMOUNT_RANGE).await?;
            type_into(device, Ids::ORDER_CREATE_FIAT_MIN, &min.to_string()).await?;
            type_into(device, Ids::ORDER_CREATE_FIAT_MAX, &max.to_string()).await
        }
    }
}

/// Sets the price type. The form starts on market price and the fixed
/// segment reveals the sats field; a range order locks the control on
/// market, since the protocol prices ranges at market only. The control
/// names the whole and has no action of its own: a segment is tapped.
///
/// Whether the form already offers a sats field is a question about the
/// whole form, not about the part of it the keyboard has not covered.
async fn enter_price(device: &DeviceContext<'_>, draft: &OrderDraft) -> Result<(), AdapterError> {
    let sats_listed = listed(device, Ids::ORDER_CREATE_SATS_AMOUNT).await?;
    match draft.price {
        OrderPrice::Fixed { amount_sats } => {
            if draft.is_range() {
                return Err(AdapterError::contract(
                    "a range order cannot carry a fixed price: the app locks it to market",
                ));
            }
            if !sats_listed {
                tap(device, Ids::ORDER_CREATE_PRICE_FIXED).await?;
            }
            type_into(
                device,
                Ids::ORDER_CREATE_SATS_AMOUNT,
                &amount_sats.to_string(),
            )
            .await
        }
        OrderPrice::Market { premium } => {
            // The premium slider is not part of the automation contract; the
            // form's default of no premium is the only market price driven.
            if premium != 0 {
                return Err(AdapterError::contract(
                    "a market premium other than zero is not part of the contract",
                ));
            }
            if sats_listed {
                tap(device, Ids::ORDER_CREATE_PRICE_MARKET).await?;
            }
            Ok(())
        }
    }
}

/// Opens the book on the side listing `order`, opens it and takes it. The
/// v2 take screen asks the fiat amount for range orders only, right after
/// the take button; a fixed order goes straight to its invoice form or
/// trade detail, so `fiat_amount` is given for a range order and only then.
pub async fn find_and_take(
    device: &DeviceContext<'_>,
    order: &OrderReference,
    kind: OrderKind,
    fiat_amount: Option<u64>,
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
    if let Some(amount) = fiat_amount {
        // The amount dialog is the app's own proof the order is a range:
        // a fixed order never shows it, and typing an amount into nothing
        // would take the order at whatever the maker set.
        expect(device, Ids::ORDER_TAKE_AMOUNT).await?;
        type_into(device, Ids::ORDER_TAKE_AMOUNT, &amount.to_string()).await?;
        tap(device, Ids::ORDER_TAKE_AMOUNT_CONFIRM).await?;
    }
    let form = match kind {
        OrderKind::Sell => Ids::INVOICE_TEXT,
        OrderKind::Buy => Ids::PAY_INVOICE_TEXT,
    };
    match super::first_present(device, &[form, Ids::ORDER_STATUS]).await? {
        Some(id) if id == form => {
            invoice_form_state(device, order).await?.ok_or_else(|| {
                AdapterError::contract("take invoice form disappeared before verification")
            })?;
            Ok(())
        }
        Some(_) => verify_order_id(device, Ids::ORDER_ID, order).await,
        None => Err(AdapterError::contract(
            "take did not reach the expected invoice form or trade detail",
        )),
    }
}

/// Rates the counterpart on the trade detail, where a completed trade
/// carries its stars and submit: one star, submit; the app stays on the
/// same trade's detail.
pub async fn rate_counterpart(
    device: &DeviceContext<'_>,
    order: &OrderReference,
    score: u8,
) -> Result<(), AdapterError> {
    if score == 0 || score > MAX_RATING {
        return Err(AdapterError::contract(format!(
            "rating score {score} is outside 1-{MAX_RATING}"
        )));
    }
    open_trade(device, order).await?;
    tap(device, &Ids::rate_star(score)).await?;
    tap(device, Ids::TRADE_RATE_SUBMIT).await?;
    expect(device, Ids::ORDER_STATUS).await?;
    verify_order_id(device, Ids::ORDER_ID, order).await
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
    if let Some(state) = invoice_form_state(device, order).await? {
        return Ok(state);
    }
    observe_detail(device, order).await
}

/// Reads the trade's state by leaving any open form and reopening the trade
/// from the list. Where it opens is the observation: the app routes a trade
/// that waits on this user's invoice or payment straight to that form, and
/// shows the detail with its status otherwise. Unlike [`observe`], a form the
/// harness itself opened earlier proves nothing here, because it is left
/// first -- so this is the read that shows the daemon's request arriving.
pub async fn observe_detail(
    device: &DeviceContext<'_>,
    order: &OrderReference,
) -> Result<AppOrderState, AdapterError> {
    go_home(device).await?;
    tap(device, Ids::NAV_TRADES).await?;
    tap(device, &Ids::trades_item(&order.id)).await?;
    let landed = super::first_present(
        device,
        &[Ids::ORDER_STATUS, Ids::INVOICE_TEXT, Ids::PAY_INVOICE_TEXT],
    )
    .await?;
    match landed {
        Some(id) if id == Ids::INVOICE_TEXT => {
            verify_order_id(device, Ids::INVOICE_ORDER_ID, order).await?;
            return Ok(AppOrderState {
                state: OrderState::Taken,
                raw_label: "waiting-invoice".into(),
            });
        }
        Some(id) if id == Ids::PAY_INVOICE_TEXT => {
            verify_order_id(device, Ids::PAY_ORDER_ID, order).await?;
            return Ok(AppOrderState {
                state: OrderState::Taken,
                raw_label: "waiting-payment".into(),
            });
        }
        Some(_) => verify_order_id(device, Ids::ORDER_ID, order).await?,
        None => return Err(AdapterError::contract("`order.status` did not appear")),
    }
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
    path: PaymentPath,
) -> Result<PaymentHandoff, AdapterError> {
    if !present(device, Ids::PAY_INVOICE_TEXT).await? {
        open_trade(device, order).await?;
        tap(device, Ids::TRADE_PAY_INVOICE).await?;
    }
    verify_order_id(device, Ids::PAY_ORDER_ID, order).await?;
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
    expect(device, Ids::ORDER_ADD_FAB).await?;
    Ok(PaymentHandoff {
        kind: PaymentPath::Nwc,
        invoice: (!invoice.is_empty()).then(|| Secret::new(invoice)),
    })
}

/// How long a just-opened screen gets to show its order id before a wrong
/// or empty readout is believed. A screen in transition (a detail route
/// pushed over a list, a rebuilt semantics node on the web) reads stale or
/// blank for a moment; a foreign form still reads foreign after it settles.
pub(super) const ID_SETTLE: std::time::Duration = std::time::Duration::from_secs(3);

pub(super) async fn verify_order_id(
    device: &DeviceContext<'_>,
    id: &str,
    order: &OrderReference,
) -> Result<(), AdapterError> {
    let deadline = tokio::time::Instant::now() + ID_SETTLE;
    loop {
        let shown = label(device, id).await?;
        if shown.trim() == order.id {
            return Ok(());
        }
        if tokio::time::Instant::now() >= deadline {
            return Err(AdapterError::contract(format!(
                "invoice or trade screen belongs to a different order: `{id}` shows `{}`, expected `{}`",
                shown.trim(),
                order.id
            )));
        }
        tokio::time::sleep(SCREEN_POLL).await;
    }
}

/// Manual invoice screens are waiting states only when their public order ID
/// matches. Reading them must not leave the form before its next user action.
async fn invoice_form_state(
    device: &DeviceContext<'_>,
    order: &OrderReference,
) -> Result<Option<AppOrderState>, AdapterError> {
    for (form, id, raw) in [
        (Ids::INVOICE_TEXT, Ids::INVOICE_ORDER_ID, "waiting-invoice"),
        (Ids::PAY_INVOICE_TEXT, Ids::PAY_ORDER_ID, "waiting-payment"),
    ] {
        if present(device, form).await? {
            verify_order_id(device, id, order).await?;
            return Ok(Some(AppOrderState {
                state: OrderState::Taken,
                raw_label: raw.into(),
            }));
        }
    }
    Ok(None)
}

/// Reports fiat sent.
pub async fn confirm_fiat_sent(
    device: &DeviceContext<'_>,
    order: &OrderReference,
) -> Result<(), AdapterError> {
    open_trade(device, order).await?;
    tap(device, Ids::TRADE_FIAT_SENT).await
}

/// Opens a dispute from the trade detail, confirming the dialog. The app
/// then moves to the dispute's own screen; the trade is reopened from the
/// list on the next observation.
pub async fn open_dispute(
    device: &DeviceContext<'_>,
    order: &OrderReference,
) -> Result<(), AdapterError> {
    open_trade(device, order).await?;
    tap(device, Ids::TRADE_DISPUTE).await?;
    tap(device, Ids::TRADE_DISPUTE_CONFIRM).await
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
    if !super::appears_within(device, &Ids::trades_item(&order.id), device.ui_timeout).await? {
        return Ok(CancelOutcome::NotFound);
    }
    tap(device, &Ids::trades_item(&order.id)).await?;
    // A trade waiting on this user's invoice or payment opens its form, not
    // the detail; the form's own cancel button cancels the order.
    let landed = super::first_present(
        device,
        &[Ids::ORDER_STATUS, Ids::INVOICE_TEXT, Ids::PAY_INVOICE_TEXT],
    )
    .await?;
    let form_cancel = match landed {
        Some(id) if id == Ids::INVOICE_TEXT => {
            verify_order_id(device, Ids::INVOICE_ORDER_ID, order).await?;
            Some(Ids::INVOICE_CANCEL)
        }
        Some(id) if id == Ids::PAY_INVOICE_TEXT => {
            verify_order_id(device, Ids::PAY_ORDER_ID, order).await?;
            Some(Ids::PAY_CANCEL)
        }
        Some(_) => None,
        None => return Err(AdapterError::contract("`order.status` did not appear")),
    };
    if let Some(cancel) = form_cancel {
        tap(device, cancel).await?;
        if super::appears_within(device, Ids::TRADE_CANCEL_CONFIRM, ID_SETTLE).await? {
            tap(device, Ids::TRADE_CANCEL_CONFIRM).await?;
        }
        return Ok(CancelOutcome::Cancelled);
    }
    let raw = label(device, Ids::ORDER_STATUS).await?;
    if from_v2_label(&raw).is_some_and(OrderState::is_terminal) {
        return Ok(CancelOutcome::AlreadyClosed);
    }
    if !present(device, Ids::TRADE_CANCEL).await? {
        // This side already asked for a cooperative cancel: the request
        // stands and only the counterpart's answer ends the trade, so the
        // detail offers no cancel of its own. (A request from the
        // counterpart leaves `trade.cancel` here, as its acceptance.)
        if present(device, Ids::TRADE_CANCEL_REQUEST).await? {
            return Ok(CancelOutcome::Cancelled);
        }
        return Err(AdapterError::contract(
            "cancel control is absent without a confirmed terminal order state",
        ));
    }
    tap(device, Ids::TRADE_CANCEL).await?;
    tap(device, Ids::TRADE_CANCEL_CONFIRM).await?;
    Ok(CancelOutcome::Cancelled)
}

/// Normalized names of the trade actions currently visible.
///
/// A trade that waits on this user's invoice or payment opens on that form,
/// not on the detail: the form is the action it asks for, and its own
/// cancel is the way out. Any other trade opens on the detail.
pub async fn visible_actions(
    device: &DeviceContext<'_>,
    order: &OrderReference,
) -> Result<Vec<String>, AdapterError> {
    go_home(device).await?;
    tap(device, Ids::NAV_TRADES).await?;
    tap(device, &Ids::trades_item(&order.id)).await?;
    let landed = super::first_present(
        device,
        &[Ids::ORDER_STATUS, Ids::INVOICE_TEXT, Ids::PAY_INVOICE_TEXT],
    )
    .await?;
    let form = match landed {
        Some(id) if id == Ids::INVOICE_TEXT => {
            Some((Ids::INVOICE_ORDER_ID, "add-invoice", Ids::INVOICE_CANCEL))
        }
        Some(id) if id == Ids::PAY_INVOICE_TEXT => {
            Some((Ids::PAY_ORDER_ID, "pay-invoice", Ids::PAY_CANCEL))
        }
        Some(_) => None,
        None => return Err(AdapterError::contract("`order.status` did not appear")),
    };
    if let Some((order_id, action, cancel)) = form {
        verify_order_id(device, order_id, order).await?;
        let mut actions = vec![action.to_owned()];
        if present(device, cancel).await? {
            actions.push("cancel".to_owned());
        }
        return Ok(actions);
    }
    detail_actions(device).await
}

/// The actions the trade detail on screen offers.
async fn detail_actions(device: &DeviceContext<'_>) -> Result<Vec<String>, AdapterError> {
    let mut actions = Vec::new();
    for (id, name) in [
        (Ids::TRADE_PAY_INVOICE, "pay-invoice"),
        (Ids::TRADE_ADD_INVOICE, "add-invoice"),
        (Ids::TRADE_FIAT_SENT, "fiat-sent"),
        (Ids::TRADE_RELEASE, "release"),
        (Ids::TRADE_CANCEL, "cancel"),
        (Ids::TRADE_DISPUTE, "dispute"),
        // A completed trade carries the rating itself; its submit is the
        // rating action.
        (Ids::TRADE_RATE_SUBMIT, "rate"),
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

pub(super) async fn page_source(device: &DeviceContext<'_>) -> Result<String, AdapterError> {
    device
        .ui
        .page_source()
        .await
        .map_err(|e| AdapterError::infrastructure(format!("page source: {e}")))
}

/// Extracts `resource-id` values with `prefix` from an accessibility XML dump.
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
