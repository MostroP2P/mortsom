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

/// Words `invoice.check` uses for an invoice the app refuses to send.
const REFUSALS: [&str; 6] = [
    "expires-too-soon",
    "wrong-amount",
    "wrong-network",
    "expired",
    "malformed",
    "unrecognized",
];
/// Words `invoice.check` uses for an invoice the app lets through.
const PASSES: [&str; 2] = ["valid", "address"];

/// The app's own reading of the typed invoice, before anything is sent.
enum Judged {
    /// It named a broken rule and disabled submit.
    Refused(String),
    /// It judged the invoice good and submit accepts a press.
    Passed(String),
    /// It had not judged it by the deadline, and submit accepts a press:
    /// its facts had not arrived, and the daemon is the only judge.
    Unjudged,
    /// The app dropped the typed text; it has to be typed again.
    Dropped,
}

/// Hands in a supplied invoice and reports whose verdict decided it.
///
/// The app judges an invoice before sending it and publishes the verdict
/// as a word in `invoice.check`; a refusal disables submit and nothing is
/// sent. The word is waited for, never read once: it is absent until the
/// facts it needs arrive. An invoice the app lets through, or has not
/// judged by the deadline, is submitted once, and the daemon decides: the
/// trade detail means taken, `invoice.error` means refused. The app's
/// refusal can also land as submit is pressed, when its facts arrive
/// just then; the form then shows that word and submit stays disabled.
pub async fn submit_buyer_invoice(
    device: &DeviceContext<'_>,
    order: &OrderReference,
    invoice: &Secret,
) -> Result<InvoiceSubmission, AdapterError> {
    open_buyer_invoice(device, order).await?;
    const ENTRIES: usize = 3;
    for _ in 0..ENTRIES {
        type_into(device, Ids::INVOICE_TEXT, invoice.expose()).await?;
        match judge(device, invoice).await? {
            Judged::Dropped => continue,
            Judged::Refused(check) => {
                verify_order_id(device, Ids::INVOICE_ORDER_ID, order).await?;
                return Ok(InvoiceSubmission::RefusedByApp { check });
            }
            Judged::Passed(check) => return send(device, order, Some(check)).await,
            Judged::Unjudged => return send(device, order, None).await,
        }
    }
    Err(AdapterError::contract(
        "buyer invoice field did not keep the supplied invoice",
    ))
}

/// Waits, up to the UI timeout, for the app's verdict on the typed text.
async fn judge(device: &DeviceContext<'_>, invoice: &Secret) -> Result<Judged, AdapterError> {
    let deadline = tokio::time::Instant::now() + device.ui_timeout;
    loop {
        if !typed(device, invoice).await? {
            return Ok(Judged::Dropped);
        }
        // A daemon refusal still on screen takes the verdict's slot: no
        // word will come while it stands, so none is waited for.
        if present(device, Ids::INVOICE_ERROR).await? {
            return Ok(Judged::Unjudged);
        }
        let check = check_word(device).await?;
        let enabled = submit_enabled(device).await?;
        match check.as_deref() {
            Some(word) if REFUSALS.contains(&word) && !enabled => {
                return Ok(Judged::Refused(word.to_owned()));
            }
            Some(word) if PASSES.contains(&word) && enabled => {
                return Ok(Judged::Passed(word.to_owned()));
            }
            Some(word) if !REFUSALS.contains(&word) && !PASSES.contains(&word) => {
                return Err(AdapterError::contract(format!(
                    "`invoice.check` shows `{word}`, which the contract does not define"
                )));
            }
            _ => {}
        }
        if tokio::time::Instant::now() >= deadline {
            return unsettled(check, enabled);
        }
        tokio::time::sleep(SCREEN_POLL).await;
    }
}

/// The verdict when the deadline passes with the word and submit still
/// disagreeing, or with no word at all.
fn unsettled(check: Option<String>, enabled: bool) -> Result<Judged, AdapterError> {
    match (check, enabled) {
        (None, true) => Ok(Judged::Unjudged),
        (None, false) => Err(AdapterError::contract(
            "`invoice.submit` stayed disabled without a verdict in `invoice.check`",
        )),
        (Some(word), true) => Err(AdapterError::contract(format!(
            "`invoice.check` shows `{word}` while `invoice.submit` stays enabled"
        ))),
        (Some(word), false) => Err(AdapterError::contract(format!(
            "`invoice.check` shows `{word}` while `invoice.submit` stays disabled"
        ))),
    }
}

/// Whether the field still holds the typed invoice.
async fn typed(device: &DeviceContext<'_>, invoice: &Secret) -> Result<bool, AdapterError> {
    let entered = device
        .ui
        .text_of(Ids::INVOICE_TEXT)
        .await
        .map_err(|_| AdapterError::contract("cannot verify buyer invoice text"))?;
    Ok(entered.trim() == invoice.expose().trim())
}

/// The word in `invoice.check`, or `None` while the app shows none.
///
/// The app withdraws the word whenever it judges the field again, and the
/// whole form when it leaves after a submission, so a word found by one
/// look can be gone by the read. A failed read of a readout that has left
/// is no word yet; one that is still there is a real failure.
async fn check_word(device: &DeviceContext<'_>) -> Result<Option<String>, AdapterError> {
    if !present(device, Ids::INVOICE_CHECK).await? {
        return Ok(None);
    }
    let word = match device.ui.label_of(Ids::INVOICE_CHECK).await {
        Ok(word) => word,
        Err(_) if !present(device, Ids::INVOICE_CHECK).await? => return Ok(None),
        Err(e) => {
            return Err(AdapterError::infrastructure(format!(
                "read `{}`: {e}",
                Ids::INVOICE_CHECK
            )));
        }
    };
    let word = word.trim().to_owned();
    Ok((!word.is_empty()).then_some(word))
}

async fn submit_enabled(device: &DeviceContext<'_>) -> Result<bool, AdapterError> {
    device
        .ui
        .is_enabled(Ids::INVOICE_SUBMIT)
        .await
        .map_err(|e| AdapterError::infrastructure(format!("probe `invoice.submit`: {e}")))
}

/// Presses submit once and reads whose verdict follows. `check` is the
/// app's word when the press was sent, carried into an acceptance.
async fn send(
    device: &DeviceContext<'_>,
    order: &OrderReference,
    check: Option<String>,
) -> Result<InvoiceSubmission, AdapterError> {
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
    let deadline = tokio::time::Instant::now() + device.ui_timeout;
    loop {
        if present(device, Ids::ORDER_STATUS).await? {
            verify_order_id(device, Ids::ORDER_ID, order).await?;
            return Ok(InvoiceSubmission::Accepted { check });
        }
        if present(device, Ids::INVOICE_ERROR).await? {
            return refused_by_daemon(device, order).await;
        }
        if let Some(word) = check_word(device).await?
            && REFUSALS.contains(&word.as_str())
            && !submit_enabled(device).await?
        {
            verify_order_id(device, Ids::INVOICE_ORDER_ID, order).await?;
            return Ok(InvoiceSubmission::RefusedByApp { check: word });
        }
        if tokio::time::Instant::now() >= deadline {
            return Err(AdapterError::contract(
                "neither the trade detail, `invoice.error` nor a refusal in `invoice.check` \
                 appeared after submitting the buyer invoice",
            ));
        }
        tokio::time::sleep(SCREEN_POLL).await;
    }
}

/// The daemon's refusal: the form stays open with its reason.
async fn refused_by_daemon(
    device: &DeviceContext<'_>,
    order: &OrderReference,
) -> Result<InvoiceSubmission, AdapterError> {
    let reason = label(device, Ids::INVOICE_ERROR).await?.trim().to_owned();
    if reason.is_empty() {
        return Err(AdapterError::contract(
            "`invoice.error` appeared without a reason",
        ));
    }
    // A refusal keeps the same form: the user fixes the invoice where
    // they typed it.
    if !present(device, Ids::INVOICE_TEXT).await? {
        return Err(AdapterError::product(
            "the app left the invoice form after the daemon refused the invoice",
        ));
    }
    verify_order_id(device, Ids::INVOICE_ORDER_ID, order).await?;
    Ok(InvoiceSubmission::Rejected { reason })
}
