//! Payments on the external-wallet path, the only one a CLI without a
//! wallet has: the seller's hold invoice is handed out for Mortsom's node to
//! pay, and the buyer's payout invoice, issued on Mortsom's node, is handed
//! in on standard input.

use std::time::Duration;

use mortsom_app_contract::{
    AdapterError, DeviceContext, InvoiceSubmission, OrderReference, PaymentHandoff, PaymentPath,
};
use mortsom_cli_driver::{Call, CliError};
use mortsom_common::Secret;

use crate::access::{adapter_error, client, try_run};
use crate::memo::Memo;
use crate::messages::{ADD_INVOICE, MessageWindow, PAY_HOLD, unix_now, wait_for};

/// Refuses the app-wallet path: the CLI has no wallet to pay with.
pub(crate) fn require_external(path: PaymentPath) -> Result<(), AdapterError> {
    match path {
        PaymentPath::ExternalWallet => Ok(()),
        PaymentPath::Nwc => Err(AdapterError::contract(
            "mostro-cli holds no wallet; only the external-wallet path exists",
        )),
    }
}

/// The hold invoice the node asked this seller to pay.
pub(crate) async fn hold_invoice(
    device: &DeviceContext<'_>,
    memo: &Memo,
    order: &OrderReference,
    path: PaymentPath,
    window: MessageWindow,
    poll: Duration,
) -> Result<PaymentHandoff, AdapterError> {
    require_external(path)?;
    let remembered = memo.get(device.actor, &order.id).hold_invoice;
    let invoice = match remembered {
        Some(invoice) => invoice,
        None => wait_for(device, &order.id, &[PAY_HOLD], None, window, poll)
            .await?
            .invoice
            .ok_or_else(|| AdapterError::contract("a hold invoice request carries no invoice"))?,
    };
    Ok(PaymentHandoff {
        kind: PaymentPath::ExternalWallet,
        invoice: Some(invoice),
    })
}

/// The sats the node asked this buyer to invoice: the take's reply, or the
/// newest request after the last invoice this buyer handed in.
pub(crate) async fn requested_payout(
    device: &DeviceContext<'_>,
    memo: &Memo,
    order: &OrderReference,
    window: MessageWindow,
    poll: Duration,
) -> Result<u64, AdapterError> {
    let remembered = memo.get(device.actor, &order.id);
    if let Some(sats) = remembered.payout_sats {
        return Ok(sats);
    }
    let request = wait_for(
        device,
        &order.id,
        &[ADD_INVOICE],
        remembered.payout_submitted_at,
        window,
        poll,
    )
    .await?;
    let sats = request
        .amount_sats
        .ok_or_else(|| AdapterError::contract("a payout request carries no amount"))?;
    memo.update(device.actor, &order.id, |r| r.payout_sats = Some(sats));
    Ok(sats)
}

fn add_invoice_call(order: &OrderReference, invoice: &Secret) -> Call {
    Call::node(&["order", "add-invoice"])
        .arg("order", order.id.clone())
        .invoice(invoice)
        .confirmed()
}

/// Hands in a payout invoice issued on Mortsom's node.
pub(crate) async fn provide(
    device: &DeviceContext<'_>,
    memo: &Memo,
    order: &OrderReference,
    invoice: Option<&Secret>,
) -> Result<Option<Secret>, AdapterError> {
    let invoice = invoice.ok_or_else(|| {
        AdapterError::contract("mostro-cli has no wallet to issue a payout invoice with")
    })?;
    client(device)?
        .invoke(add_invoice_call(order, invoice))
        .await
        .map_err(adapter_error)?;
    submitted(memo, device, order);
    Ok(Some(invoice.clone()))
}

/// Hands in `invoice` and reports whose verdict decided it.
pub(crate) async fn submit(
    device: &DeviceContext<'_>,
    memo: &Memo,
    order: &OrderReference,
    invoice: &Secret,
) -> Result<InvoiceSubmission, AdapterError> {
    match try_run(device, add_invoice_call(order, invoice)).await? {
        Ok(_) => {
            submitted(memo, device, order);
            Ok(InvoiceSubmission::Accepted { check: None })
        }
        Err(error) => verdict_of(error),
    }
}

/// A refusal is an answer here: the CLI's own check before sending, or the
/// node's after it.
fn verdict_of(error: CliError) -> Result<InvoiceSubmission, AdapterError> {
    match error.code() {
        // Only the CLI's own invoice check is the app refusing the invoice;
        // any other argument refusal is a contract mismatch.
        Some("invalid_arguments") if error.reason() == Some("invalid_invoice") => {
            Ok(InvoiceSubmission::RefusedByApp {
                check: "invalid-invoice".into(),
            })
        }
        Some("node_refused") => Ok(InvoiceSubmission::Rejected {
            reason: error.reason().unwrap_or("node_refused").to_owned(),
        }),
        _ => Err(adapter_error(error)),
    }
}

/// After an accepted invoice the next request, if any, must be newer.
fn submitted(memo: &Memo, device: &DeviceContext<'_>, order: &OrderReference) {
    let now = i64::try_from(unix_now()).unwrap_or(i64::MAX);
    memo.update(device.actor, &order.id, |r| {
        r.payout_sats = None;
        r.payout_submitted_at = Some(now);
    });
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    fn refused(code: &str, reason: Option<&str>) -> CliError {
        CliError::Refused {
            command: "order.add-invoice".into(),
            code: code.into(),
            message: "fixed".into(),
            reason: reason.map(str::to_owned),
        }
    }

    #[test]
    fn the_node_and_the_cli_refusals_are_answers() {
        assert_eq!(
            verdict_of(refused("node_refused", Some("invalid_invoice"))),
            Ok(InvoiceSubmission::Rejected {
                reason: "invalid_invoice".into()
            })
        );
        assert_eq!(
            verdict_of(refused("invalid_arguments", Some("invalid_invoice"))),
            Ok(InvoiceSubmission::RefusedByApp {
                check: "invalid-invoice".into()
            })
        );
        let other_argument = verdict_of(refused("invalid_arguments", None)).unwrap_err();
        assert_eq!(
            other_argument.category,
            mortsom_common::FailureCategory::AutomationContract
        );
        assert!(verdict_of(refused("relay_connection_failed", None)).is_err());
    }

    #[test]
    fn only_the_external_wallet_path_exists() {
        assert!(require_external(PaymentPath::ExternalWallet).is_ok());
        assert!(require_external(PaymentPath::Nwc).is_err());
    }
}
