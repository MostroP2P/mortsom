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
    pub const ALL: [OrderState; 8] = [
        OrderState::Pending,
        OrderState::Taken,
        OrderState::Active,
        OrderState::FiatSent,
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
            FiatSent => matches!(next, Completed | Disputed | Cancelled),
            Disputed => matches!(next, Completed | Cancelled),
            Completed | Cancelled | Expired => false,
        }
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
    fn happy_path_is_legal_and_skipping_payment_is_not() {
        use OrderState::*;
        let path = [Pending, Taken, Active, FiatSent, Completed];
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
