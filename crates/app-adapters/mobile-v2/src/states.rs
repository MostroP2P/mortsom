//! Mapping from the v2 app's displayed trade status to the normalized
//! business states. The v2 trade detail derives its own `TradeStatus`
//! (loading, pending, waiting-invoice, waiting-payment, in-progress,
//! active, fiat-sent, payout-pending, completed, cancelled, disputed, pending-rating,
//! rated); `order.status` exposes that machine name. Protocol statuses are
//! accepted too so a build that forwards them still maps.

use mortsom_app_contract::OrderState;

/// Label the v2 app shows before it resolved the status; not a state.
pub const LOADING: &str = "loading";

/// Maps a v2 status label; `None` for `loading` or an unknown label.
pub fn from_v2_label(label: &str) -> Option<OrderState> {
    let normalized = label.trim().to_ascii_lowercase().replace(['_', ' '], "-");
    Some(match normalized.as_str() {
        LOADING => return None,
        // `waiting-bond` is the anti-abuse bond window: taken, the trade
        // flow not started.
        "waiting-invoice" | "waiting-payment" | "waiting-bond" | "in-progress" => OrderState::Taken,
        "payout-pending" => OrderState::PayoutPending,
        "completed" | "pending-rating" | "rated" => OrderState::Completed,
        "cancelled" => OrderState::Cancelled,
        "disputed" => OrderState::Disputed,
        other => return OrderState::from_protocol_status(other),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_v2_status_maps_or_is_loading() {
        let cases = [
            ("pending", Some(OrderState::Pending)),
            ("waiting-invoice", Some(OrderState::Taken)),
            ("waiting-payment", Some(OrderState::Taken)),
            ("in-progress", Some(OrderState::Taken)),
            ("waiting-bond", Some(OrderState::Taken)),
            ("Waiting Bond", Some(OrderState::Taken)),
            ("active", Some(OrderState::Active)),
            ("fiat-sent", Some(OrderState::FiatSent)),
            ("payout-pending", Some(OrderState::PayoutPending)),
            ("completed", Some(OrderState::Completed)),
            ("pending-rating", Some(OrderState::Completed)),
            ("rated", Some(OrderState::Completed)),
            ("cancelled", Some(OrderState::Cancelled)),
            ("canceled", Some(OrderState::Cancelled)),
            ("cooperatively-canceled", Some(OrderState::Cancelled)),
            ("disputed", Some(OrderState::Disputed)),
            ("dispute", Some(OrderState::Disputed)),
            ("expired", Some(OrderState::Expired)),
            ("loading", None),
            ("Waiting Payment", Some(OrderState::Taken)),
            ("pending_rating", Some(OrderState::Completed)),
            ("something-else", None),
        ];
        for (label, expected) in cases {
            assert_eq!(from_v2_label(label), expected, "{label}");
        }
    }

    #[test]
    #[allow(clippy::expect_used)]
    fn escrow_settlement_and_failed_payout_never_mean_completed() {
        for label in [
            "payout-pending",
            "Payout Pending",
            "settled-hold-invoice",
            "payment-failed",
        ] {
            let state = from_v2_label(label).expect("payout state must be recognized");
            assert_eq!(state.as_str(), "payout-pending");
            assert!(!state.is_terminal());
        }
    }
}
