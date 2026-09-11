//! Exact payout invoice preparation before the public submission action.
use mortsom_app_contract::OrderReference;
use mortsom_common::Actor;
use mortsom_common::Secret;
use mortsom_observers::wallet::{amount_msats_of, expiry_secs_of, payment_hash_of};

use crate::{Context, ScenarioError, journal::RecoveryEvent};

/// Which invoice the buyer's node issues for the payout.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BuyerInvoice {
    /// An ordinary invoice the daemon can pay at once.
    Standard,
    /// An invoice that stops being payable `secs` after issue; below the
    /// daemon's declared floor it is refused at submission.
    Expiring {
        /// Lifetime in seconds.
        secs: u64,
    },
    /// A hold invoice: the daemon's payment is accepted and then held,
    /// unresolved, until the scenario settles it.
    Hold,
}

pub(super) struct Prepared {
    pub invoice: Option<Secret>,
    pub(super) expected: Option<(String, u64)>,
}

impl Prepared {
    /// The hash the node issued, when Mortsom holds the buyer's node.
    pub(super) fn hash(&self) -> Option<&str> {
        self.expected.as_ref().map(|(hash, _)| hash.as_str())
    }
}

pub(super) async fn prepare(
    ctx: &Context,
    actor: Actor,
    order: &OrderReference,
    kind: BuyerInvoice,
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
    // A fixed order's payout can never exceed the sats the maker named. An
    // order the daemon prices (market, range) has no such number: there the
    // bound is the per-order spend cap the operator set.
    let cap = if order.priced_by_daemon {
        ctx.budget
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .caps()
            .max_order_sats
    } else {
        ctx.amount_sats
    };
    validate_request(requested, cap)?;
    let memo = format!("mortsom {}", order.id);
    let invoice = match kind {
        BuyerInvoice::Standard => node.make_invoice(requested, &memo).await,
        BuyerInvoice::Expiring { secs } => node.make_invoice_expiring(requested, &memo, secs).await,
        BuyerInvoice::Hold => node.make_hold_invoice(requested, &memo).await,
    }
    .map_err(ScenarioError::environment)?;
    ctx.protect(&invoice);
    let hash = validate_invoice(&invoice, requested)?;
    if let BuyerInvoice::Expiring { secs } = kind {
        validate_expiry(&invoice, secs)?;
    }
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

/// The node must have encoded the lifetime that was asked for: a scenario
/// about the daemon's expiry floor proves nothing with a default lifetime.
fn validate_expiry(invoice: &Secret, secs: u64) -> Result<(), ScenarioError> {
    let encoded =
        expiry_secs_of(invoice).map_err(|_| ScenarioError::wallet("invalid buyer invoice"))?;
    if encoded != secs {
        return Err(ScenarioError::wallet(format!(
            "buyer invoice encodes a {encoded}s lifetime, not the {secs}s requested"
        )));
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

    /// The reference invoice encodes a one-minute lifetime; asking for
    /// anything else must be caught before submission.
    #[test]
    fn a_requested_lifetime_the_node_did_not_encode_is_refused() {
        let invoice = Secret::new(INVOICE);
        assert!(validate_expiry(&invoice, 60).is_ok());
        let error = validate_expiry(&invoice, 120).unwrap_err();
        assert!(error.message.contains("60s lifetime"), "{error}");
        assert!(validate_expiry(&Secret::new("not-an-invoice"), 120).is_err());
    }
}
