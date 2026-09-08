//! Exact principal conversion and conservative fee accounting.
use super::{LndError, LndPaymentState};
use fedimint_tonic_lnd::lnrpc;

pub(super) fn whole_sats(label: &str, field: &'static str, msats: i64) -> Result<u64, LndError> {
    if msats < 0 || msats % 1000 != 0 {
        return Err(LndError::InvalidAmount {
            label: label.into(),
            field,
            value: msats.into(),
        });
    }
    Ok((msats / 1000) as u64)
}

pub(super) fn fees(label: &str, msats: i64) -> Result<u64, LndError> {
    u64::try_from(msats)
        .map(|value| value.div_ceil(1000))
        .map_err(|_| LndError::InvalidAmount {
            label: label.into(),
            field: "routing fee millisatoshi",
            value: msats.into(),
        })
}

pub(super) fn outgoing(
    label: &str,
    payment: &lnrpc::Payment,
) -> Result<(u64, u64, LndPaymentState), LndError> {
    let state = match lnrpc::payment::PaymentStatus::try_from(payment.status) {
        Ok(lnrpc::payment::PaymentStatus::Succeeded) => LndPaymentState::Settled,
        Ok(lnrpc::payment::PaymentStatus::Failed) => LndPaymentState::Failed,
        Ok(lnrpc::payment::PaymentStatus::Initiated | lnrpc::payment::PaymentStatus::InFlight) => {
            LndPaymentState::Pending
        }
        _ => return Err(invalid_state(label, "outgoing payment")),
    };
    Ok((
        whole_sats(label, "outgoing principal millisatoshi", payment.value_msat)?,
        fees(label, payment.fee_msat)?,
        state,
    ))
}

pub(super) fn incoming(
    label: &str,
    invoice: &lnrpc::Invoice,
) -> Result<(u64, LndPaymentState), LndError> {
    let state = match lnrpc::invoice::InvoiceState::try_from(invoice.state) {
        Ok(lnrpc::invoice::InvoiceState::Settled) => LndPaymentState::Settled,
        Ok(lnrpc::invoice::InvoiceState::Canceled) => LndPaymentState::Failed,
        Ok(lnrpc::invoice::InvoiceState::Open | lnrpc::invoice::InvoiceState::Accepted) => {
            LndPaymentState::Pending
        }
        _ => return Err(invalid_state(label, "incoming invoice")),
    };
    let msats = if state == LndPaymentState::Settled {
        invoice.amt_paid_msat
    } else {
        invoice.value_msat
    };
    Ok((
        whole_sats(label, "incoming principal millisatoshi", msats)?,
        state,
    ))
}

fn invalid_state(label: &str, operation: &'static str) -> LndError {
    LndError::Failed {
        label: label.into(),
        operation,
        message: "unknown LND state".into(),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    #[test]
    fn principal_is_exact_and_fees_never_understate_a_fractional_sat() {
        assert_eq!(whole_sats("a", "principal", 999_000).unwrap(), 999);
        assert!(whole_sats("a", "principal", 999_001).is_err());
        assert!(whole_sats("a", "principal", -1).is_err());
        assert_eq!(fees("a", 1001).unwrap(), 2);
        assert!(fees("a", -1).is_err());
    }
    #[test]
    fn settled_invoice_reports_actual_msats_without_falling_back_to_face_value() {
        let mut invoice = lnrpc::Invoice {
            state: lnrpc::invoice::InvoiceState::Settled as i32,
            value: 999,
            value_msat: 999_000,
            amt_paid_sat: 999,
            amt_paid_msat: 999_001,
            ..Default::default()
        };
        assert!(incoming("buyer", &invoice).is_err());
        invoice.amt_paid_msat = 0;
        assert_eq!(
            incoming("buyer", &invoice).unwrap(),
            (0, LndPaymentState::Settled)
        );
        invoice.state = lnrpc::invoice::InvoiceState::Accepted as i32;
        assert_eq!(
            incoming("buyer", &invoice).unwrap(),
            (999, LndPaymentState::Pending)
        );
    }
    #[test]
    fn unknown_outgoing_states_are_errors_instead_of_pending_payments() {
        assert!(
            outgoing(
                "seller",
                &lnrpc::Payment {
                    status: 999,
                    ..Default::default()
                }
            )
            .is_err()
        );
    }
}
