//! The fixture daemon's bond side: the winner's claim and its payout,
//! the loser's notice, and the bonds a solver's ruling slashes.
use super::*;

impl Adapter {
    /// The claim on `order` the app offers `actor`, if any.
    pub(super) fn claim_for(
        &self,
        actor: Actor,
        order: &str,
    ) -> Result<Option<BondClaim>, AdapterError> {
        let state = self.state.lock().unwrap();
        Ok(state
            .claims
            .get(order)
            .filter(|claim| claim.actor == actor)
            .map(|claim| BondClaim {
                amount_sats: claim.amount,
                status: claim.phase.clone(),
            }))
    }

    /// `actor` hands in `invoice` for their claim on `order`: the claim
    /// reads submitted, and after the daemon's delay the payout lands and
    /// it reads completed. A hold invoice holds the payment instead, until
    /// the winner's node settles or cancels it.
    pub(super) fn take_claim_invoice(
        &self,
        actor: Actor,
        order: &str,
        invoice: &Secret,
    ) -> Result<String, AdapterError> {
        self.action("claim", actor);
        let hash = payment_hash_of(invoice).unwrap();
        {
            let mut state = self.state.lock().unwrap();
            let Some(claim) = state.claims.get(order).filter(|c| c.actor == actor) else {
                return Err(AdapterError::product(format!(
                    "the app offers no claim for order {order}"
                )));
            };
            if claim.phase != "pending" {
                return Err(AdapterError::product(format!(
                    "the claim on order {order} is already {}",
                    claim.phase
                )));
            }
            let paid = state.invoice_mut(&hash);
            assert!(paid.claim, "a claim is paid to a claim invoice");
            paid.order = Some(order.to_owned());
            state.claims.get_mut(order).unwrap().phase = "submitted".into();
        }
        let state = self.state.clone();
        let order = order.to_owned();
        tokio::spawn(async move {
            tokio::time::sleep(DAEMON_DELAY).await;
            let mut guard = state.lock().unwrap();
            let invoice = guard.invoice_mut(&hash);
            if invoice.hold.is_some() {
                invoice.hold = Some(HoldInvoiceState::Accepted);
                guard.actions.push("daemon:pay-claim-held".into());
                guard.claims.get_mut(&order).unwrap().phase = "acknowledged".into();
                return;
            }
            invoice.settled = true;
            guard.actions.push("daemon:pay-claim".into());
            guard.claims.get_mut(&order).unwrap().phase = "completed".into();
        });
        Ok("submitted".into())
    }

    /// The cause `actor`'s trade on `order` shows for their slashed bond,
    /// while the trade is still listed.
    pub(super) fn slash_notice_for(&self, actor: Actor, order: &str) -> Option<String> {
        let state = self.state.lock().unwrap();
        if state.declined(actor, order) || state.dropped.contains(&order.to_owned()) {
            return None;
        }
        state
            .slash_notice
            .get(order)
            .filter(|(loser, _)| *loser == actor)
            .map(|(_, cause)| (*cause).to_owned())
    }
}

impl Adapter {
    /// The daemon's deadline for `duty` on `order`: when the fixture daemon
    /// slashes on a timeout, a duty still undone once it elapses slashes
    /// whoever owed it, and the book shows what that leaves.
    pub(crate) fn start_clock(state: &Arc<Mutex<State>>, order: &str, duty: Duty) {
        if !state.lock().unwrap().slashes_on_timeout {
            return;
        }
        let state = state.clone();
        let order = order.to_owned();
        tokio::spawn(async move {
            tokio::time::sleep(DEADLINE).await;
            let shown = state.lock().unwrap().time_out(&order, duty);
            if let Some(wire) = shown {
                Adapter::for_state(&state).publish(&order, wire, &[]).await;
            }
        });
    }

    /// The deadline of the bond just asked for on `order`, when the fixture
    /// daemon lets unpaid bonds lapse: a maker's order it never published is
    /// dropped without a word, a taker's take ends and the order, `pending`
    /// all along, is republished.
    pub(crate) fn lapse_unpaid_bond(&self, order: &str, taker: bool) {
        if !self.state.lock().unwrap().expires_unpaid_bonds {
            return;
        }
        let this = self.detached();
        let order = order.to_owned();
        tokio::spawn(async move {
            tokio::time::sleep(DEADLINE).await;
            let republish = {
                let mut state = this.state.lock().unwrap();
                let (requested, payer) = if taker {
                    (state.bond_requested.remove(&order), state.taker())
                } else {
                    (state.maker_bond_requested.remove(&order), state.maker())
                };
                let Some(payer) = payer.filter(|_| requested) else {
                    return;
                };
                if !(taker && state.misbehaviour.keeps_lapsed_takes) {
                    state.declined.push((payer, order.clone()));
                }
                state.actions.push(format!("daemon:lapse-bond:{payer}"));
                if taker {
                    state.set(&order, OrderState::Pending);
                }
                taker
            };
            if republish {
                this.publish(&order, "pending", &[]).await;
            }
        });
    }

    /// The bonds a ruling slashes, resolved to the parties of the order.
    pub(super) fn apply_slash(&self, order_id: &str, slash: mortsom_scenario_kit::BondSlash) {
        let mut state = self.state.lock().unwrap();
        let buyer = state.buyer().expect("an order names its buyer");
        let seller = State::counterparty(buyer);
        state.actions.push(format!(
            "solver:slash seller={} buyer={}",
            slash.seller, slash.buyer
        ));
        if slash.buyer {
            state.slash(order_id, buyer, "dispute");
        }
        if slash.seller {
            state.slash(order_id, seller, "dispute");
        }
    }
}
