//! Normalized business states (spec §12.1). Adapters map their own
//! rendering to these; the scenario kit asserts only on them.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

/// User-visible order state, independent of any application generation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum OrderState {
    /// Published, waiting for a taker.
    Pending,
    /// Taken; a bond, hold invoice or buyer invoice is still outstanding.
    Taken,
    /// Hold invoice paid; fiat exchange in progress.
    Active,
    /// Buyer reported the fiat payment.
    FiatSent,
    /// Seller released escrow; the buyer payout is still unresolved.
    PayoutPending,
    /// Seller released; sats settled to the buyer.
    Completed,
    /// Cancelled before completion.
    Cancelled,
    /// Expired by the daemon.
    Expired,
    /// A dispute is open or resolved by a solver.
    Disputed,
}

impl OrderState {
    /// Every state, in lifecycle order.
    pub const ALL: [OrderState; 9] = [
        OrderState::Pending,
        OrderState::Taken,
        OrderState::Active,
        OrderState::FiatSent,
        OrderState::PayoutPending,
        OrderState::Completed,
        OrderState::Cancelled,
        OrderState::Expired,
        OrderState::Disputed,
    ];

    /// Stable name used in reports.
    pub fn as_str(self) -> &'static str {
        match self {
            OrderState::Pending => "pending",
            OrderState::Taken => "taken",
            OrderState::Active => "active",
            OrderState::FiatSent => "fiat-sent",
            OrderState::PayoutPending => "payout-pending",
            OrderState::Completed => "completed",
            OrderState::Cancelled => "cancelled",
            OrderState::Expired => "expired",
            OrderState::Disputed => "disputed",
        }
    }

    /// True for states no legal transition leaves.
    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            OrderState::Completed | OrderState::Cancelled | OrderState::Expired
        )
    }

    /// Whether `next` may legally follow `self` on the user-visible path
    /// (spec §8: no regression from a terminal state, no skipping payment).
    pub fn may_transition_to(self, next: OrderState) -> bool {
        use OrderState::*;
        if self == next {
            return true;
        }
        match self {
            Pending => matches!(next, Taken | Cancelled | Expired),
            Taken => matches!(next, Active | Cancelled | Expired | Pending),
            Active => matches!(next, FiatSent | Cancelled | Disputed | Expired),
            FiatSent => matches!(next, PayoutPending | Completed | Disputed | Cancelled),
            PayoutPending => matches!(next, Completed),
            Disputed => matches!(next, PayoutPending | Completed | Cancelled),
            Completed | Cancelled | Expired => false,
        }
    }
}

impl OrderState {
    /// Maps a status label to the normalized state. Wire statuses come from
    /// mostro-core (`s` tag of kind 38383 or the app's readout of the same
    /// value); `payment-failed` is an app-local readout (the buyer payout
    /// failed after release), accepted so the adapter never mistakes it for
    /// an unknown state. Escrow settlement and a failed buyer payment stay
    /// nonterminal: the payout may still complete after a daemon retry.
    pub fn from_protocol_status(status: &str) -> Option<OrderState> {
        Some(match status.trim() {
            "pending" => OrderState::Pending,
            "waiting-buyer-invoice"
            | "waiting-payment"
            | "waiting-taker-bond"
            | "waiting-maker-bond"
            | "in-progress" => OrderState::Taken,
            "active" => OrderState::Active,
            "fiat-sent" => OrderState::FiatSent,
            "payout-pending" | "settled-hold-invoice" | "payment-failed" => {
                OrderState::PayoutPending
            }
            "success" | "completed-by-admin" | "settled-by-admin" => OrderState::Completed,
            "canceled" | "cooperatively-canceled" | "canceled-by-admin" => OrderState::Cancelled,
            "expired" => OrderState::Expired,
            "dispute" => OrderState::Disputed,
            _ => return None,
        })
    }
}

impl fmt::Display for OrderState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for OrderState {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        OrderState::ALL
            .into_iter()
            .find(|state| state.as_str() == s)
            .ok_or_else(|| format!("unknown order state `{s}`"))
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn names_round_trip() {
        for state in OrderState::ALL {
            assert_eq!(state.as_str().parse::<OrderState>().unwrap(), state);
            assert_eq!(
                serde_json::to_string(&state).unwrap(),
                format!("\"{state}\"")
            );
        }
        assert!("done".parse::<OrderState>().is_err());
    }

    #[test]
    fn protocol_statuses_map_to_durable_states() {
        assert_eq!(
            OrderState::from_protocol_status("pending"),
            Some(OrderState::Pending)
        );
        assert_eq!(
            OrderState::from_protocol_status("waiting-payment"),
            Some(OrderState::Taken)
        );
        assert_eq!(
            OrderState::from_protocol_status(" active "),
            Some(OrderState::Active)
        );
        assert_eq!(
            OrderState::from_protocol_status("settled-hold-invoice"),
            Some(OrderState::PayoutPending)
        );
        assert_eq!(
            OrderState::from_protocol_status("success"),
            Some(OrderState::Completed)
        );
        assert_eq!(
            OrderState::from_protocol_status("cooperatively-canceled"),
            Some(OrderState::Cancelled)
        );
        assert_eq!(
            OrderState::from_protocol_status("dispute"),
            Some(OrderState::Disputed)
        );
        assert_eq!(OrderState::from_protocol_status("bogus"), None);
    }

    #[test]
    fn terminal_states_never_regress() {
        for terminal in [
            OrderState::Completed,
            OrderState::Cancelled,
            OrderState::Expired,
        ] {
            assert!(terminal.is_terminal());
            for next in OrderState::ALL {
                assert_eq!(
                    terminal.may_transition_to(next),
                    terminal == next,
                    "{terminal}->{next}"
                );
            }
        }
    }

    #[test]
    fn escrow_release_is_nonterminal_and_cannot_regress_to_fiat_exchange() {
        let pending = OrderState::from_protocol_status("settled-hold-invoice").unwrap();
        assert_eq!(pending.as_str(), "payout-pending");
        assert!(!pending.is_terminal());
        assert!(OrderState::FiatSent.may_transition_to(pending));
        assert!(OrderState::Disputed.may_transition_to(pending));
        assert!(pending.may_transition_to(OrderState::Completed));
        for invalid in [
            OrderState::FiatSent,
            OrderState::Active,
            OrderState::Cancelled,
        ] {
            assert!(!pending.may_transition_to(invalid));
        }
        for label in ["payout-pending", "payment-failed"] {
            assert_eq!(OrderState::from_protocol_status(label), Some(pending));
        }
    }

    #[test]
    fn happy_path_is_legal_and_skipping_payment_is_not() {
        use OrderState::*;
        let path = [Pending, Taken, Active, FiatSent, PayoutPending, Completed];
        for pair in path.windows(2) {
            assert!(pair[0].may_transition_to(pair[1]), "{pair:?}");
        }
        assert!(!Pending.may_transition_to(Active));
        assert!(!Taken.may_transition_to(FiatSent));
        assert!(!Pending.may_transition_to(Completed));
        assert!(
            Taken.may_transition_to(Pending),
            "taker bond timeout returns to pending"
        );
    }
}
