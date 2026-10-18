//! Cap accounting (spec Phase 0.5, §5, §16.1). Every wallet action must
//! reserve against this budget first. A missing cap is an error, never an
//! unlimited default; a reservation whose fate is unknown stays reserved.

use mortsom_common::FailureCategory;
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Per-order and per-run spend caps, always both present.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpendCaps {
    /// Maximum sats any single order may move.
    pub max_order_sats: u64,
    /// Maximum sats the whole run may move, fees included.
    pub max_run_spend_sats: u64,
}

impl SpendCaps {
    /// Builds caps from configuration values; zero is treated as missing.
    pub fn new(max_order_sats: u64, max_run_spend_sats: u64) -> Result<Self, BudgetError> {
        if max_order_sats == 0 {
            return Err(BudgetError::MissingCap("run.max_order_sats"));
        }
        if max_run_spend_sats == 0 {
            return Err(BudgetError::MissingCap("run.max_run_spend_sats"));
        }
        Ok(Self {
            max_order_sats,
            max_run_spend_sats,
        })
    }
}

/// Budget failures.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum BudgetError {
    /// A cap is absent or zero.
    #[error("missing spend cap {0}; a missing cap is never unlimited")]
    MissingCap(&'static str),
    /// A single reservation exceeds the per-order cap.
    #[error("order {order_id}: {requested} sats exceeds per-order cap {cap}")]
    ExceedsOrderCap {
        /// Order the reservation belongs to.
        order_id: String,
        /// Requested sats.
        requested: u64,
        /// Cap.
        cap: u64,
    },
    /// The reservation would push the run past its cap.
    #[error(
        "order {order_id}: {requested} sats exceeds remaining run budget {remaining} (cap {cap})"
    )]
    ExceedsRunCap {
        /// Order the reservation belongs to.
        order_id: String,
        /// Requested sats.
        requested: u64,
        /// Remaining sats before this request.
        remaining: u64,
        /// Cap.
        cap: u64,
    },
    /// The settled principal exceeds what was reserved. Routing fees are
    /// not reservable in advance; they count against the run cap only.
    #[error("reservation {reservation}: settled {settled} exceeds reserved {reserved} (fee {fee})")]
    Overspend {
        /// Reservation id.
        reservation: u64,
        /// Settled sats.
        settled: u64,
        /// Fee sats.
        fee: u64,
        /// Reserved sats.
        reserved: u64,
    },
    /// The reservation id is unknown, already resolved, or belongs to a
    /// different order than the handle claims.
    #[error("unknown, resolved or mismatched reservation {0}")]
    UnknownReservation(u64),
    /// The reservation counter is exhausted.
    #[error("reservation ids exhausted")]
    Exhausted,
}

impl BudgetError {
    /// Budget refusals are configuration/environment problems.
    pub fn category(&self) -> FailureCategory {
        FailureCategory::EnvironmentMisconfiguration
    }
}

/// A live reservation. Must be resolved with `commit`, `release` or
/// `mark_in_flight`; dropping it does not free the budget. It is an opaque
/// handle: only [`Budget::reserve`] can create one, and every use is
/// cross-checked against the ledger entry's order.
#[must_use = "a Reservation must be resolved with commit, release or mark_in_flight"]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reservation {
    id: u64,
    order_id: String,
    sats: u64,
}

impl Reservation {
    /// Reservation id.
    pub fn id(&self) -> u64 {
        self.id
    }

    /// Order the reservation belongs to.
    pub fn order_id(&self) -> &str {
        &self.order_id
    }

    /// Reserved sats.
    pub fn sats(&self) -> u64 {
        self.sats
    }
}

/// Terminal state of a ledger entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum EntryStatus {
    /// Reserved, not yet resolved.
    Reserved,
    /// Payment settled; sats and fee are final.
    Settled,
    /// Payment definitively failed; nothing moved.
    Released,
    /// Fate unknown; the reservation is kept forever for safety.
    InFlight,
}

/// One ledger entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LedgerEntry {
    /// Reservation id.
    pub id: u64,
    /// Order the entry belongs to.
    pub order_id: String,
    /// Reserved sats.
    pub reserved_sats: u64,
    /// Settled sats (0 until settled).
    pub settled_sats: u64,
    /// Fee sats (0 until settled).
    pub fee_sats: u64,
    /// Payment hash, when known.
    pub payment_hash: Option<String>,
    /// Status.
    pub status: EntryStatus,
}

/// Aggregate totals for reports.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Totals {
    /// Sats currently reserved (unresolved + in-flight).
    pub reserved_sats: u64,
    /// Sats settled.
    pub settled_sats: u64,
    /// Fees settled.
    pub fee_sats: u64,
    /// Sats still available under the run cap.
    pub remaining_sats: u64,
}

/// The run budget and its ledger. All methods take `&mut self`; concurrent
/// scenarios share one budget behind `Arc<Mutex<Budget>>`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Budget {
    caps: SpendCaps,
    next_id: u64,
    ledger: Vec<LedgerEntry>,
}

impl Budget {
    /// Creates an empty budget under `caps`.
    pub fn new(caps: SpendCaps) -> Self {
        Self {
            caps,
            next_id: 1,
            ledger: Vec::new(),
        }
    }

    /// The caps in force.
    pub fn caps(&self) -> SpendCaps {
        self.caps
    }

    /// The ledger, in reservation order.
    pub fn ledger(&self) -> &[LedgerEntry] {
        &self.ledger
    }

    /// Sats counted against the run cap: reserved plus settled plus fees.
    fn used(&self) -> u64 {
        self.used_where(|_| true)
    }

    /// Sats an order already holds or spent, across every reservation it
    /// took: an order with a bond and an escrow holds both.
    fn used_by(&self, order_id: &str) -> u64 {
        self.used_where(|entry| entry.order_id == order_id)
    }

    fn used_where(&self, keep: impl Fn(&LedgerEntry) -> bool) -> u64 {
        self.ledger
            .iter()
            .filter(|entry| keep(entry))
            .map(|e| match e.status {
                EntryStatus::Reserved | EntryStatus::InFlight => e.reserved_sats,
                EntryStatus::Settled => e.settled_sats + e.fee_sats,
                EntryStatus::Released => 0,
            })
            .sum()
    }

    /// Reserves `sats` for `order_id`, refusing before any wallet action if
    /// either cap would be exceeded. The per-order cap bounds everything
    /// the order holds together, not each reservation on its own.
    pub fn reserve(&mut self, order_id: &str, sats: u64) -> Result<Reservation, BudgetError> {
        let held = self.used_by(order_id).saturating_add(sats);
        if held > self.caps.max_order_sats {
            return Err(BudgetError::ExceedsOrderCap {
                order_id: order_id.to_owned(),
                requested: held,
                cap: self.caps.max_order_sats,
            });
        }
        let remaining = self.caps.max_run_spend_sats.saturating_sub(self.used());
        if sats > remaining {
            return Err(BudgetError::ExceedsRunCap {
                order_id: order_id.to_owned(),
                requested: sats,
                remaining,
                cap: self.caps.max_run_spend_sats,
            });
        }
        let id = self.next_id;
        self.next_id = self.next_id.checked_add(1).ok_or(BudgetError::Exhausted)?;
        self.ledger.push(LedgerEntry {
            id,
            order_id: order_id.to_owned(),
            reserved_sats: sats,
            settled_sats: 0,
            fee_sats: 0,
            payment_hash: None,
            status: EntryStatus::Reserved,
        });
        Ok(Reservation {
            id,
            order_id: order_id.to_owned(),
            sats,
        })
    }

    fn entry_mut(&mut self, reservation: &Reservation) -> Result<&mut LedgerEntry, BudgetError> {
        self.ledger
            .iter_mut()
            .find(|e| {
                e.id == reservation.id
                    && e.order_id == reservation.order_id
                    && e.reserved_sats == reservation.sats
                    && matches!(e.status, EntryStatus::Reserved | EntryStatus::InFlight)
            })
            .ok_or(BudgetError::UnknownReservation(reservation.id))
    }

    /// Records a settled payment. The principal may not exceed the
    /// reservation; the routing fee is counted against the run cap.
    pub fn commit(
        &mut self,
        reservation: &Reservation,
        settled_sats: u64,
        fee_sats: u64,
        payment_hash: Option<String>,
    ) -> Result<(), BudgetError> {
        let entry = self.entry_mut(reservation)?;
        if settled_sats > entry.reserved_sats {
            return Err(BudgetError::Overspend {
                reservation: reservation.id,
                settled: settled_sats,
                fee: fee_sats,
                reserved: entry.reserved_sats,
            });
        }
        entry.settled_sats = settled_sats;
        entry.fee_sats = fee_sats;
        entry.payment_hash = payment_hash;
        entry.status = EntryStatus::Settled;
        Ok(())
    }

    /// Frees a reservation whose payment definitively failed.
    pub fn release(&mut self, reservation: &Reservation) -> Result<(), BudgetError> {
        self.entry_mut(reservation)?.status = EntryStatus::Released;
        Ok(())
    }

    /// Keeps a reservation whose fate is unknown (an HTLC may still settle).
    pub fn mark_in_flight(
        &mut self,
        reservation: &Reservation,
        payment_hash: Option<String>,
    ) -> Result<(), BudgetError> {
        let entry = self.entry_mut(reservation)?;
        entry.payment_hash = payment_hash;
        entry.status = EntryStatus::InFlight;
        Ok(())
    }

    /// Aggregate totals.
    pub fn totals(&self) -> Totals {
        let reserved_sats = self
            .ledger
            .iter()
            .filter(|e| matches!(e.status, EntryStatus::Reserved | EntryStatus::InFlight))
            .map(|e| e.reserved_sats)
            .sum();
        let settled_sats = self
            .ledger
            .iter()
            .filter(|e| e.status == EntryStatus::Settled)
            .map(|e| e.settled_sats)
            .sum();
        let fee_sats = self
            .ledger
            .iter()
            .filter(|e| e.status == EntryStatus::Settled)
            .map(|e| e.fee_sats)
            .sum();
        Totals {
            reserved_sats,
            settled_sats,
            fee_sats,
            remaining_sats: self.caps.max_run_spend_sats.saturating_sub(self.used()),
        }
    }

    /// True when at least one reservation is unresolved or in flight.
    pub fn has_open_reservations(&self) -> bool {
        self.ledger
            .iter()
            .any(|e| matches!(e.status, EntryStatus::Reserved | EntryStatus::InFlight))
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    fn caps() -> SpendCaps {
        SpendCaps {
            max_order_sats: 1000,
            max_run_spend_sats: 2500,
        }
    }

    #[test]
    fn zero_caps_are_missing_caps() {
        assert_eq!(
            SpendCaps::new(0, 10).unwrap_err(),
            BudgetError::MissingCap("run.max_order_sats")
        );
        assert_eq!(
            SpendCaps::new(10, 0).unwrap_err(),
            BudgetError::MissingCap("run.max_run_spend_sats")
        );
        assert_eq!(
            SpendCaps::new(10, 20).unwrap(),
            SpendCaps {
                max_order_sats: 10,
                max_run_spend_sats: 20
            }
        );
    }

    #[test]
    fn reserve_refuses_over_order_cap_before_touching_the_run_cap() {
        let mut budget = Budget::new(caps());

        let err = budget.reserve("o1", 1001).unwrap_err();

        assert!(matches!(
            err,
            BudgetError::ExceedsOrderCap {
                requested: 1001,
                cap: 1000,
                ..
            }
        ));
        assert!(budget.ledger().is_empty());
        assert_eq!(err.category(), FailureCategory::EnvironmentMisconfiguration);
    }

    #[test]
    fn reserve_refuses_when_run_cap_would_be_exceeded() {
        let mut budget = Budget::new(caps());
        let _a = budget.reserve("o1", 1000).unwrap();
        let _b = budget.reserve("o2", 1000).unwrap();

        let err = budget.reserve("o3", 600).unwrap_err();

        assert!(matches!(
            err,
            BudgetError::ExceedsRunCap {
                requested: 600,
                remaining: 500,
                cap: 2500,
                ..
            }
        ));
        assert_eq!(budget.totals().remaining_sats, 500);
    }

    #[test]
    fn commit_counts_settled_plus_fee_and_frees_the_rest() {
        let mut budget = Budget::new(caps());
        let r = budget.reserve("o1", 1000).unwrap();

        budget.commit(&r, 900, 10, Some("hash".into())).unwrap();

        let totals = budget.totals();
        assert_eq!(totals.settled_sats, 900);
        assert_eq!(totals.fee_sats, 10);
        assert_eq!(totals.reserved_sats, 0);
        assert_eq!(totals.remaining_sats, 2500 - 910);
        assert_eq!(budget.ledger()[0].status, EntryStatus::Settled);
        assert_eq!(budget.ledger()[0].payment_hash.as_deref(), Some("hash"));
        assert!(!budget.has_open_reservations());
    }

    #[test]
    fn commit_refuses_overspend_and_keeps_the_reservation() {
        let mut budget = Budget::new(caps());
        let r = budget.reserve("o1", 500).unwrap();

        // Fees never trip the per-order check; the principal does.
        let err = budget.commit(&r, 501, 0, None).unwrap_err();

        assert!(matches!(err, BudgetError::Overspend { .. }));
        assert_eq!(budget.ledger()[0].status, EntryStatus::Reserved);
        assert!(budget.has_open_reservations());
    }

    #[test]
    fn release_frees_budget_and_in_flight_keeps_it() {
        let mut budget = Budget::new(caps());
        let a = budget.reserve("o1", 1000).unwrap();
        let b = budget.reserve("o2", 1000).unwrap();

        budget.release(&a).unwrap();
        budget.mark_in_flight(&b, Some("h".into())).unwrap();

        let totals = budget.totals();
        assert_eq!(totals.reserved_sats, 1000);
        assert_eq!(totals.remaining_sats, 1500);
        assert!(budget.has_open_reservations());
        // An in-flight reservation may still be committed once its fate is known.
        budget.commit(&b, 1000, 0, None).unwrap();
        assert!(!budget.has_open_reservations());
    }

    #[test]
    fn resolved_reservations_cannot_be_reused() {
        let mut budget = Budget::new(caps());
        let r = budget.reserve("o1", 100).unwrap();
        budget.release(&r).unwrap();

        assert_eq!(
            budget.commit(&r, 1, 0, None).unwrap_err(),
            BudgetError::UnknownReservation(1)
        );
        assert_eq!(
            budget.release(&r).unwrap_err(),
            BudgetError::UnknownReservation(1)
        );
    }

    #[test]
    fn a_forged_reservation_for_another_order_is_rejected() {
        let mut budget = Budget::new(caps());
        let real = budget.reserve("o1", 100).unwrap();
        let forged = Reservation {
            id: real.id(),
            order_id: "o2".into(),
            sats: real.sats(),
        };

        assert_eq!(
            budget.commit(&forged, 1, 0, None).unwrap_err(),
            BudgetError::UnknownReservation(1)
        );
        assert_eq!(budget.ledger()[0].status, EntryStatus::Reserved);
        assert_eq!(real.order_id(), "o1");
        budget.release(&real).unwrap();
    }

    /// A bonded order holds its escrow and its bond at once; together they
    /// stay under the per-order cap, and a released hold no longer counts.
    #[test]
    fn the_per_order_cap_bounds_everything_one_order_holds_together() {
        let mut budget = Budget::new(SpendCaps::new(1000, 10_000).unwrap());
        let escrow = budget.reserve("o1", 700).unwrap();
        let err = budget.reserve("o1", 400).unwrap_err();
        assert!(matches!(
            err,
            BudgetError::ExceedsOrderCap {
                requested: 1100,
                cap: 1000,
                ..
            }
        ));
        let bond = budget.reserve("o1", 300).unwrap();
        // Another order is bounded on its own.
        assert!(budget.reserve("o2", 1000).is_ok());
        budget.release(&bond).unwrap();
        assert!(budget.reserve("o1", 300).is_ok());
        budget.commit(&escrow, 700, 1, None).unwrap();
        assert!(budget.reserve("o1", 1).is_err(), "settled sats still count");
    }

    #[test]
    fn used_never_exceeds_run_cap_across_mixed_sequences() {
        let mut budget = Budget::new(caps());
        let mut open = Vec::new();
        for step in 0..200u64 {
            let sats = 1 + (step * 37) % 1000;
            match budget.reserve(&format!("o{step}"), sats) {
                Ok(r) => open.push(r),
                Err(BudgetError::ExceedsRunCap { .. }) => {}
                Err(other) => panic!("{other}"),
            }
            if step % 3 == 0 {
                if let Some(r) = open.pop() {
                    budget.commit(&r, r.sats() / 2, r.sats() / 4, None).unwrap();
                }
            } else if step % 3 == 1 {
                if let Some(r) = open.pop() {
                    budget.release(&r).unwrap();
                }
            } else if let Some(r) = open.pop() {
                budget.mark_in_flight(&r, None).unwrap();
            }
            let totals = budget.totals();
            let used = totals.reserved_sats + totals.settled_sats + totals.fee_sats;
            assert!(
                used <= caps().max_run_spend_sats,
                "step {step}: used {used}"
            );
            assert_eq!(totals.remaining_sats, caps().max_run_spend_sats - used);
        }
    }

    #[test]
    fn budget_serializes_for_accounting_artifacts() {
        let mut budget = Budget::new(caps());
        let _r = budget.reserve("o1", 10).unwrap();

        let json = serde_json::to_string(&budget).unwrap();

        assert!(json.contains("\"status\":\"reserved\""));
        assert!(json.contains("\"max_run_spend_sats\":2500"));
    }
}
