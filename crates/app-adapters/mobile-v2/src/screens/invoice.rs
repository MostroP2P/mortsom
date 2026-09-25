//! The buyer's invoice form in the v2 app: open it for one exact order,
//! read the amount the daemon asks for, type an invoice and hand it in,
//! either expecting it to be taken or asking for the verdict.

use mortsom_app_contract::{AdapterError, DeviceContext, InvoiceSubmission, OrderReference};
use mortsom_common::Secret;

use super::orders::{ID_SETTLE, open_trade, verify_order_id};
use super::{SCREEN_POLL, expect, label, present, tap, type_into};
use crate::selectors::Ids;

/// Whether the typed invoice is still in the field and submit accepts a
/// press, within a bounded settle. `false` when the app dropped the text
/// or never enabled submit, so the caller can type it again.
async fn invoice_ready(device: &DeviceContext<'_>, invoice: &Secret) -> Result<bool, AdapterError> {
    let deadline = tokio::time::Instant::now() + ID_SETTLE;
    loop {
        let entered = device
            .ui
            .text_of(Ids::INVOICE_TEXT)
            .await
            .map_err(|_| AdapterError::contract("cannot verify buyer invoice text"))?;
        if entered.trim() != invoice.expose().trim() {
            return Ok(false);
        }
        if device
            .ui
            .is_enabled(Ids::INVOICE_SUBMIT)
            .await
            .map_err(|e| AdapterError::infrastructure(format!("probe `invoice.submit`: {e}")))?
        {
            return Ok(true);
        }
        if tokio::time::Instant::now() >= deadline {
            return Ok(false);
        }
        tokio::time::sleep(SCREEN_POLL).await;
    }
}

/// Opens the exact order's manual invoice form without replacing an existing
/// form for that same order. Its public order ID binds the amount to the trade.
async fn open_buyer_invoice(
    device: &DeviceContext<'_>,
    order: &OrderReference,
) -> Result<(), AdapterError> {
    if present(device, Ids::INVOICE_TEXT).await?
        && present(device, Ids::INVOICE_ORDER_ID).await?
        && label(device, Ids::INVOICE_ORDER_ID).await?.trim() == order.id
    {
        return Ok(());
    }
    open_trade(device, order).await?;
    tap(device, Ids::TRADE_ADD_INVOICE).await?;
    if present(device, Ids::INVOICE_MANUAL).await? {
        tap(device, Ids::INVOICE_MANUAL).await?;
    }
    expect(device, Ids::INVOICE_TEXT).await?;
    if label(device, Ids::INVOICE_ORDER_ID).await?.trim() != order.id {
        return Err(AdapterError::contract(
            "buyer invoice form belongs to a different order",
        ));
    }
    Ok(())
}

/// Reads the daemon-requested sats shown to the user before an invoice exists.
pub async fn buyer_invoice_amount(
    device: &DeviceContext<'_>,
    order: &OrderReference,
) -> Result<u64, AdapterError> {
    open_buyer_invoice(device, order).await?;
    let raw = label(device, Ids::INVOICE_AMOUNT).await?;
    let raw = raw.trim();
    if raw.is_empty() || !raw.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(AdapterError::contract(
            "buyer invoice amount is not unsigned whole sats",
        ));
    }
    raw.parse::<u64>()
        .ok()
        .filter(|amount| *amount > 0)
        .ok_or_else(|| {
            AdapterError::product("buyer invoice amount is zero or outside the supported range")
        })
}

/// Submits a supplied invoice through the public manual form, or uses the
/// existing connected-wallet flow when no invoice is supplied.
pub async fn provide_buyer_invoice(
    device: &DeviceContext<'_>,
    order: &OrderReference,
    supplied: Option<&Secret>,
) -> Result<Option<Secret>, AdapterError> {
    if let Some(invoice) = supplied {
        enter_buyer_invoice(device, order, invoice).await?;
        tap(device, Ids::INVOICE_SUBMIT).await?;
        expect(device, Ids::ORDER_STATUS).await?;
        if label(device, Ids::ORDER_ID).await?.trim() != order.id {
            return Err(AdapterError::contract(
                "buyer invoice returned to a different trade",
            ));
        }
        return Ok(Some(invoice.clone()));
    }
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

/// Opens the manual form for `order` and types `invoice` into it, leaving
/// submit enabled and untouched.
///
/// The daemon's request for the invoice can arrive while the user is
/// already on the form (the maker of a buy lands there before the
/// seller's hold is accepted); the app rebuilds the form and drops what
/// was typed. Typing is not a business action, so the invoice is entered
/// again, a bounded number of times. Submit stays single-press.
async fn enter_buyer_invoice(
    device: &DeviceContext<'_>,
    order: &OrderReference,
    invoice: &Secret,
) -> Result<(), AdapterError> {
    open_buyer_invoice(device, order).await?;
    const ENTRIES: usize = 3;
    for _ in 0..ENTRIES {
        type_into(device, Ids::INVOICE_TEXT, invoice.expose()).await?;
        if invoice_ready(device, invoice).await? {
            return Ok(());
        }
    }
    Err(AdapterError::contract(
        "buyer invoice field did not keep the supplied invoice with submit enabled",
    ))
}

/// Submits a supplied invoice once and reports the daemon's verdict: the
/// trade detail means it was taken; the form kept open with `invoice.error`
/// showing means it was refused, and the reason is that readout.
pub async fn submit_buyer_invoice(
    device: &DeviceContext<'_>,
    order: &OrderReference,
    invoice: &Secret,
) -> Result<InvoiceSubmission, AdapterError> {
    enter_buyer_invoice(device, order, invoice).await?;
    // A form kept open after a refusal still shows that refusal's
    // `invoice.error`; the app clears it when it handles the new
    // submission. Reading the verdict before it clears would report the
    // previous refusal as this one's.
    let stale_error = present(device, Ids::INVOICE_ERROR).await?;
    tap(device, Ids::INVOICE_SUBMIT).await?;
    if stale_error && !super::gone_within(device, Ids::INVOICE_ERROR, device.ui_timeout).await? {
        return Err(AdapterError::contract(
            "the previous `invoice.error` never cleared after a new submission",
        ));
    }
    match super::first_present(device, &[Ids::ORDER_STATUS, Ids::INVOICE_ERROR]).await? {
        Some(id) if id == Ids::ORDER_STATUS => {
            verify_order_id(device, Ids::ORDER_ID, order).await?;
            Ok(InvoiceSubmission::Accepted)
        }
        Some(_) => {
            let reason = label(device, Ids::INVOICE_ERROR).await?.trim().to_owned();
            if reason.is_empty() {
                return Err(AdapterError::contract(
                    "`invoice.error` appeared without a reason",
                ));
            }
            // A refusal keeps the same form: the user fixes the invoice
            // where they typed it.
            if !present(device, Ids::INVOICE_TEXT).await? {
                return Err(AdapterError::product(
                    "the app left the invoice form after the daemon refused the invoice",
                ));
            }
            verify_order_id(device, Ids::INVOICE_ORDER_ID, order).await?;
            Ok(InvoiceSubmission::Rejected { reason })
        }
        None => Err(AdapterError::contract(
            "neither the trade detail nor `invoice.error` appeared after submitting the buyer invoice",
        )),
    }
}
