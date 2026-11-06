//! What the node already told an actor about an order in a command's reply:
//! the hold or bond invoice a take returned, the payout amount it asked
//! for. The same requests are also on the relay (`message list`), which is
//! where the adapter looks when it has no reply in hand.

use std::collections::HashMap;
use std::sync::{Mutex, MutexGuard, PoisonError};

use mortsom_common::{Actor, Secret};

/// The requests remembered for one actor and order.
#[derive(Debug, Clone, Default)]
pub(crate) struct Requests {
    /// The hold invoice the seller must pay.
    pub hold_invoice: Option<Secret>,
    /// The anti-abuse bond invoice this actor must pay.
    pub bond_invoice: Option<Secret>,
    /// Sats the node asked the buyer to invoice.
    pub payout_sats: Option<u64>,
    /// Unix seconds of this actor's last accepted payout invoice: a request
    /// for a new one must be newer.
    pub payout_submitted_at: Option<i64>,
    /// Unix seconds of this actor's last handed-in claim invoice.
    pub claim_submitted_at: Option<i64>,
}

/// Requests per actor and order.
#[derive(Debug, Default)]
pub(crate) struct Memo(Mutex<HashMap<(String, String), Requests>>);

impl Memo {
    /// A copy of what is remembered.
    pub(crate) fn get(&self, actor: Actor, order_id: &str) -> Requests {
        self.lock()
            .get(&key(actor, order_id))
            .cloned()
            .unwrap_or_default()
    }

    /// Changes what is remembered.
    pub(crate) fn update(&self, actor: Actor, order_id: &str, change: impl FnOnce(&mut Requests)) {
        change(self.lock().entry(key(actor, order_id)).or_default());
    }

    fn lock(&self) -> MutexGuard<'_, HashMap<(String, String), Requests>> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

fn key(actor: Actor, order_id: &str) -> (String, String) {
    (actor.as_str().to_owned(), order_id.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requests_are_kept_per_actor_and_order() {
        let memo = Memo::default();
        memo.update(Actor::Bob, "o1", |r| r.payout_sats = Some(9900));

        assert_eq!(memo.get(Actor::Bob, "o1").payout_sats, Some(9900));
        assert_eq!(memo.get(Actor::Alice, "o1").payout_sats, None);
        assert_eq!(memo.get(Actor::Bob, "o2").payout_sats, None);
    }
}
