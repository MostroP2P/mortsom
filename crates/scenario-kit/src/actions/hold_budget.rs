//! Exact invoice principal plus the payment edge's enforced routing-fee ceiling.
use crate::context::ScenarioError;
use mortsom_common::Secret;
pub(super) fn quote(invoice: &Secret, fee_cap: u64) -> Result<(u64, u64), ScenarioError> {
    let msats = mortsom_observers::wallet::amount_msats_of(invoice)
        .map_err(|e| ScenarioError::wallet(e.to_string()))?
        .ok_or_else(|| ScenarioError::wallet("hold invoice has no fixed amount"))?;
    quote_msats(msats, fee_cap)
}
fn quote_msats(msats: u64, fee_cap: u64) -> Result<(u64, u64), ScenarioError> {
    if msats == 0 || !msats.is_multiple_of(1000) {
        return Err(ScenarioError::wallet(
            "hold invoice amount must be positive whole sats",
        ));
    }
    let amount = msats / 1000;
    let reserve = amount.checked_add(fee_cap).ok_or_else(|| {
        ScenarioError::wallet("hold invoice plus routing fee exceeds the amount range")
    })?;
    Ok((amount, reserve))
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn reserves_the_actual_escrow_and_enforced_fee_without_rounding() {
        assert_eq!(quote_msats(1_005_000, 50), Ok((1005, 1055)));
        for amount in [0, 10, 1001] {
            assert!(quote_msats(amount, 50).is_err());
        }
        assert!(quote_msats(1000, u64::MAX).is_err());
    }
}

/// Owns a reservation until the first operation that may initiate payment.
/// Dropping preparation before that boundary releases it, including cancellation.
pub(super) struct PreparedReservation<'a> {
    ctx: &'a crate::Context,
    reservation: crate::budget::Reservation,
    release_on_drop: bool,
}

impl<'a> PreparedReservation<'a> {
    pub(super) fn reserve(
        ctx: &'a crate::Context,
        order_id: &str,
        sats: u64,
    ) -> Result<Self, ScenarioError> {
        let reservation = ctx
            .budget
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .reserve(order_id, sats)
            .map_err(|error| ScenarioError::environment(error.to_string()))?;
        Ok(Self {
            ctx,
            reservation,
            release_on_drop: true,
        })
    }

    pub(super) fn requote(&mut self, invoice: &Secret, fee_cap: u64) -> Result<u64, ScenarioError> {
        if !self.release_on_drop {
            return Err(ScenarioError::environment(
                "cannot resize a reservation after payment may have started",
            ));
        }
        let (amount, reserved) = quote(invoice, fee_cap)?;
        let mut budget = self.ctx.budget.lock().unwrap_or_else(|p| p.into_inner());
        budget
            .release(&self.reservation)
            .map_err(|error| ScenarioError::environment(error.to_string()))?;
        self.release_on_drop = false;
        self.reservation = budget
            .reserve(self.reservation.order_id(), reserved)
            .map_err(|error| ScenarioError::environment(error.to_string()))?;
        self.release_on_drop = true;
        Ok(amount)
    }

    pub(super) fn payment_may_start(&mut self, hash: Option<String>) -> Result<(), ScenarioError> {
        self.ctx
            .budget
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .mark_in_flight(&self.reservation, hash)
            .map_err(|error| ScenarioError::environment(error.to_string()))?;
        self.release_on_drop = false;
        Ok(())
    }

    pub(super) fn into_reservation(mut self) -> crate::budget::Reservation {
        self.release_on_drop = false;
        self.reservation.clone()
    }
}

impl Drop for PreparedReservation<'_> {
    fn drop(&mut self) {
        if self.release_on_drop {
            let result = self
                .ctx
                .budget
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .release(&self.reservation);
            if let Err(error) = result {
                self.ctx.timeline.push(
                    crate::timeline::TimelineKind::Step,
                    None,
                    format!("prepayment budget release failed: {error}"),
                    None,
                );
            }
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod reservation_tests {
    use super::*;
    use crate::{
        budget::EntryStatus,
        test_support::{StateAdapter, context},
    };
    use mortsom_app_contract::OrderState;
    use std::sync::{Arc, Mutex};

    fn ctx() -> crate::Context {
        context(Arc::new(StateAdapter(Mutex::new(OrderState::Taken))))
    }

    #[test]
    fn invalid_invoice_before_payment_releases_preparation_budget() {
        let ctx = ctx();
        {
            let mut prepared = PreparedReservation::reserve(&ctx, "o1", 1).unwrap();
            assert!(
                prepared
                    .requote(&Secret::new("invalid-invoice"), 0)
                    .is_err()
            );
        }
        assert!(!ctx.budget.lock().unwrap().has_open_reservations());
    }

    #[test]
    fn a_refused_requote_releases_the_initial_reservation_without_leaking_a_replacement() {
        let ctx = ctx();
        let invoice = Secret::new(
            "lnbc2500u1pvjluezsp5zyg3zyg3zyg3zyg3zyg3zyg3zyg3zyg3zyg3zyg3zyg3zyg3zygspp5qqqsyqcyq5rqwzqfqqqsyqcyq5rqwzqfqqqsyqcyq5rqwzqfqypqdq5xysxxatsyp3k7enxv4jsxqzpu9qrsgquk0rl77nj30yxdy8j9vdx85fkpmdla2087ne0xh8nhedh8w27kyke0lp53ut353s06fv3qfegext0eh0ymjpf39tuven09sam30g4vgpfna3rh",
        );
        {
            let mut prepared = PreparedReservation::reserve(&ctx, "o1", 1).unwrap();
            assert!(prepared.requote(&invoice, 50).is_err());
        }
        let budget = ctx.budget.lock().unwrap();
        assert!(!budget.has_open_reservations());
        assert_eq!(budget.ledger().len(), 1);
    }

    #[tokio::test]
    async fn canceling_preparation_releases_budget_but_canceling_payment_keeps_it() {
        let ctx = ctx();
        for may_start in [false, true] {
            let preparing = async {
                let mut prepared = PreparedReservation::reserve(&ctx, "o1", 1).unwrap();
                if may_start {
                    prepared.payment_may_start(Some("hash".into())).unwrap();
                }
                std::future::pending::<()>().await;
            };
            assert!(
                tokio::time::timeout(std::time::Duration::from_millis(5), preparing)
                    .await
                    .is_err()
            );
            assert_eq!(
                ctx.budget.lock().unwrap().has_open_reservations(),
                may_start
            );
        }
        let budget = ctx.budget.lock().unwrap();
        assert_eq!(budget.ledger()[1].status, EntryStatus::InFlight);
        assert_eq!(budget.ledger()[1].payment_hash.as_deref(), Some("hash"));
    }
}
