//! Exact payout invoice preparation before the public submission action.
use mortsom_android_driver::avd::Actor;
use mortsom_app_contract::OrderReference;
use mortsom_common::Secret;
use mortsom_observers::wallet::{amount_msats_of, payment_hash_of};

use crate::{Context, ScenarioError, journal::RecoveryEvent};

pub(super) struct Prepared {
    pub invoice: Option<Secret>,
    expected: Option<(String, u64)>,
}

pub(super) async fn prepare(
    ctx: &Context,
    actor: Actor,
    order: &OrderReference,
) -> Result<Prepared, ScenarioError> {
    let Some(node) = ctx.node_wallet(actor) else {
        return Ok(Prepared {
            invoice: None,
            expected: None,
        });
    };
    let session = ctx.actor(actor)?;
    let device = session.device(ctx.ui_timeout);
    let requested = ctx.adapter.buyer_invoice_amount(&device, order).await?;
    // A market-priced order is sized by the daemon, not by the run's fixed
    // amount, so the bound is the per-order spend cap the operator set.
    let cap = ctx
        .budget
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .caps()
        .max_order_sats;
    validate_request(requested, cap)?;
    let invoice = node
        .make_invoice(requested, &format!("mortsom {}", order.id))
        .await
        .map_err(ScenarioError::environment)?;
    ctx.protect(&invoice);
    let hash = validate_invoice(&invoice, requested)?;
    ctx.journal
        .record(RecoveryEvent::PaymentExpected {
            order_id: order.id.clone(),
            wallet: actor.as_str().into(),
            hash: hash.clone(),
            incoming: true,
            amount_sats: requested,
        })
        .map_err(ScenarioError::environment)?;
    if ctx.adapter.buyer_invoice_amount(&device, order).await? != requested {
        return Err(ScenarioError::product(
            "requested buyer invoice amount changed before submission",
        ));
    }
    Ok(Prepared {
        invoice: Some(invoice),
        expected: Some((hash, requested)),
    })
}

fn validate_request(requested: u64, cap: u64) -> Result<(), ScenarioError> {
    if requested == 0 || requested > cap || requested.checked_mul(1000).is_none() {
        return Err(ScenarioError::product(
            "requested buyer invoice amount is zero or exceeds the order cap",
        ));
    }
    Ok(())
}

fn validate_invoice(invoice: &Secret, requested: u64) -> Result<String, ScenarioError> {
    let amount =
        amount_msats_of(invoice).map_err(|_| ScenarioError::wallet("invalid buyer invoice"))?;
    let expected = requested
        .checked_mul(1000)
        .ok_or_else(|| ScenarioError::wallet("buyer invoice amount overflows millisatoshis"))?;
    if amount != Some(expected) {
        return Err(ScenarioError::wallet(
            "buyer invoice does not encode the exact requested amount",
        ));
    }
    payment_hash_of(invoice)
        .map_err(|_| ScenarioError::wallet("invalid buyer invoice payment hash"))
}

pub(super) fn verify_returned(
    prepared: &Prepared,
    returned: Option<&Secret>,
) -> Result<Option<String>, ScenarioError> {
    match (&prepared.expected, returned) {
        (Some((expected_hash, amount)), Some(invoice)) => {
            let hash = validate_invoice(invoice, *amount)?;
            if &hash != expected_hash {
                return Err(ScenarioError::product(
                    "app returned a different buyer invoice payment hash",
                ));
            }
            Ok(Some(hash))
        }
        (Some(_), None) => Err(ScenarioError::product(
            "app did not return the supplied buyer invoice",
        )),
        (None, Some(invoice)) => payment_hash_of(invoice)
            .map(Some)
            .map_err(|_| ScenarioError::product("app returned an invalid buyer invoice")),
        (None, None) => Ok(None),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    const INVOICE: &str = "lnbc2500u1pvjluezsp5zyg3zyg3zyg3zyg3zyg3zyg3zyg3zyg3zyg3zyg3zyg3zyg3zygspp5qqqsyqcyq5rqwzqfqqqsyqcyq5rqwzqfqqqsyqcyq5rqwzqfqypqdq5xysxxatsyp3k7enxv4jsxqzpu9qrsgquk0rl77nj30yxdy8j9vdx85fkpmdla2087ne0xh8nhedh8w27kyke0lp53ut353s06fv3qfegext0eh0ymjpf39tuven09sam30g4vgpfna3rh";

    #[test]
    fn requested_amount_can_deduct_fees_but_cannot_exceed_the_order_cap() {
        assert!(validate_request(999, 1000).is_ok());
        for amount in [0, 1001, u64::MAX] {
            assert!(validate_request(amount, 1000).is_err());
        }
    }

    #[test]
    fn returned_invoice_must_match_both_the_supplied_hash_and_exact_amount() {
        let invoice = Secret::new(INVOICE);
        let hash = validate_invoice(&invoice, 250_000).unwrap();
        let mut prepared = Prepared {
            invoice: Some(invoice.clone()),
            expected: Some((hash.clone(), 250_000)),
        };
        assert_eq!(
            verify_returned(&prepared, Some(&invoice)).unwrap(),
            Some(hash)
        );
        assert!(verify_returned(&prepared, None).is_err());
        prepared.expected = Some(("different hash".into(), 250_000));
        assert!(verify_returned(&prepared, Some(&invoice)).is_err());
        prepared.expected = Some((payment_hash_of(&invoice).unwrap(), 249_999));
        assert!(verify_returned(&prepared, Some(&invoice)).is_err());
        let error = validate_invoice(&Secret::new("secret-invalid-invoice"), 1).unwrap_err();
        assert!(!error.message.contains("secret-invalid-invoice"));
    }
}
