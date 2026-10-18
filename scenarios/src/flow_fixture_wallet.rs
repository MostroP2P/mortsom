//! The fixture's nodes: the invoices they issue, the payments they make,
//! and what each payment reports as its fate.
use super::*;

pub(super) struct Wallet {
    pub(super) actor: Actor,
    pub(super) state: Arc<Mutex<State>>,
}

impl Wallet {
    /// Issues an invoice on this node and remembers it by hash.
    fn issue(&self, sats: u64, expiry_secs: u64, hold: bool) -> Secret {
        let mut state = self.state.lock().unwrap();
        // A winner's node issues the claim invoice for the share on offer;
        // every other invoice is Bob's payout for the whole order.
        let share = BOND_POLICY.counterparty_share_sats(BOND_POLICY.bond_sats(SATS));
        let claim = sats == share && state.claims.values().any(|c| c.actor == self.actor);
        if !claim {
            assert_eq!(self.actor, Actor::Bob);
            assert_eq!(sats, SATS);
            // The maker of a buy is asked for the invoice only once the
            // seller's hold is accepted; the taker of a sell hands it in first.
            if matches!(state.created, Some((_, OrderKind::Buy))) {
                assert!(
                    !state.funded.is_empty(),
                    "buyer invoice must follow accepted escrow"
                );
            }
        }
        state.issued += 1;
        let invoice = Secret::new(build_invoice(sats * 1000, expiry_secs, state.issued));
        let hash = payment_hash_of(&invoice).unwrap();
        state.invoices.insert(
            hash,
            IssuedInvoice {
                actor: self.actor,
                order: None,
                hold: hold.then_some(HoldInvoiceState::Open),
                superseded: false,
                settled: false,
                bond: false,
                claim,
            },
        );
        invoice
    }
}

#[async_trait]
impl NodeWallet for Wallet {
    async fn make_invoice(&self, sats: u64, _: &str) -> Result<Secret, String> {
        Ok(self.issue(sats, 3600, false))
    }
    async fn make_invoice_expiring(
        &self,
        sats: u64,
        _: &str,
        expiry_secs: u64,
    ) -> Result<Secret, String> {
        Ok(self.issue(sats, expiry_secs, false))
    }
    async fn make_hold_invoice(&self, sats: u64, _: &str) -> Result<Secret, String> {
        Ok(self.issue(sats, 3600, true))
    }
    async fn cancel_invoice(&self, hash: &str) -> Result<(), String> {
        let mut state = self.state.lock().unwrap();
        let actor = self.actor;
        state.actions.push(format!("cancel-invoice:{actor}"));
        let invoice = state.invoice_mut(hash);
        assert_eq!(invoice.actor, actor);
        if invoice.settled {
            return Err("a settled invoice cannot be cancelled".into());
        }
        let held_claim = invoice.claim && invoice.hold == Some(HoldInvoiceState::Accepted);
        let order = invoice.order.clone();
        invoice.superseded = true;
        invoice.hold = invoice.hold.map(|_| HoldInvoiceState::Cancelled);
        if let (true, Some(order)) = (held_claim, order) {
            // The daemon's payment of the claim fails; its one retry spent,
            // it asks the winner for a new invoice.
            let state = self.state.clone();
            tokio::spawn(async move {
                tokio::time::sleep(DAEMON_DELAY).await;
                let mut guard = state.lock().unwrap();
                guard.actions.push("daemon:reprompt-claim".into());
                guard.claims.get_mut(&order).unwrap().phase = "pending".into();
            });
        }
        Ok(())
    }
    async fn settled_record_count(&self, hash: &str, incoming: bool) -> Result<usize, String> {
        assert!(incoming);
        let state = self.state.lock().unwrap();
        Ok(usize::from(state.invoices[hash].settled))
    }
    async fn hold_invoice_state(&self, hash: &str) -> Result<HoldInvoiceState, String> {
        let state = self.state.lock().unwrap();
        state.invoices[hash]
            .hold
            .ok_or_else(|| "not a hold invoice".to_owned())
    }
    async fn settle_hold_invoice(&self, hash: &str) -> Result<(), String> {
        let order = {
            let mut state = self.state.lock().unwrap();
            let actor = self.actor;
            state.actions.push(format!("settle-hold:{actor}"));
            let invoice = state.invoice_mut(hash);
            if invoice.hold != Some(HoldInvoiceState::Accepted) {
                return Err("no held payment to settle".into());
            }
            invoice.hold = Some(HoldInvoiceState::Settled);
            invoice.settled = true;
            invoice
                .order
                .clone()
                .expect("a held payout names its order")
        };
        Adapter::payout_landed(&self.state, &order).await;
        Ok(())
    }
    async fn start_payment(&self, invoice: &Secret) -> Result<(), String> {
        let hash = payment_hash_of(invoice).unwrap();
        if self.state.lock().unwrap().invoices[&hash].bond {
            return self.lock_bond(&hash).await;
        }
        assert_eq!(self.actor, Actor::Alice);
        let mut state = self.state.lock().unwrap();
        state.actions.push("pay:alice".into());
        if state.misbehaviour.fail_payment {
            return Err("injected hold payment failure".into());
        }
        let order = state.invoices[&hash]
            .order
            .clone()
            .expect("a hold invoice names its order");
        state.funded.insert(order.clone());
        // On a sell the daemon tells both parties `active` once the hold
        // is accepted; on a buy it first asks the maker for the invoice,
        // against the clock.
        if matches!(state.created, Some((_, OrderKind::Sell))) {
            state.set(&order, OrderState::Active);
        } else {
            drop(state);
            Adapter::start_clock(&self.state, &order, Duty::BuyerInvoice);
        }
        Ok(())
    }
}

impl Wallet {
    /// The taker's bond payment reaches the daemon: the bond locks, the
    /// take is published as in progress, and the trade flow starts.
    async fn lock_bond(&self, hash: &str) -> Result<(), String> {
        let (order, maker, kind) = {
            let mut state = self.state.lock().unwrap();
            let actor = self.actor;
            assert_eq!(state.invoices[hash].actor, actor);
            state.actions.push(format!("pay-bond:{actor}"));
            let order = state.invoices[hash]
                .order
                .clone()
                .expect("a bond names its order");
            let maker = state.maker_bond_requested.remove(&order);
            if !maker {
                assert!(
                    state.bond_requested.remove(&order),
                    "no bond was requested for {order}"
                );
            }
            (order, maker, state.created.map(|(_, kind)| kind))
        };
        let daemon = Adapter::for_state(&self.state);
        if maker {
            // The maker's bond locks: the order is published at last.
            let tags = daemon.book_tags();
            daemon.publish(&order, "pending", &tags).await;
            return Ok(());
        }
        daemon.publish(&order, "in-progress", &[]).await;
        // The trade's first step, against the clock: a sell's buyer
        // invoices, a buy's seller funds.
        let duty = match kind {
            Some(OrderKind::Buy) => Duty::Escrow,
            _ => Duty::BuyerInvoice,
        };
        Adapter::start_clock(&self.state, &order, duty);
        Ok(())
    }
}
#[async_trait]
impl WalletReader for Wallet {
    async fn lookup(&self, hash: &str) -> Result<Option<(u64, u64, PaymentState)>, String> {
        let mut state = self.state.lock().unwrap();
        let invoice = state.invoices[hash].clone();
        let payment_state = if invoice.bond {
            Self::bond_fate(&state, &invoice)
        } else if invoice.claim {
            if invoice.settled {
                PaymentState::Settled
            } else if invoice.superseded {
                PaymentState::Failed
            } else {
                PaymentState::Pending
            }
        } else {
            Self::fate(self.actor, &state, &invoice)
        };
        state.wallet_checks.push(format!(
            "{}:{}",
            self.actor,
            match payment_state {
                PaymentState::Settled => "settled",
                PaymentState::Failed => "failed",
                _ => "pending",
            }
        ));
        let amount = if invoice.bond {
            BOND_POLICY.bond_sats(SATS)
        } else if invoice.claim {
            BOND_POLICY.counterparty_share_sats(BOND_POLICY.bond_sats(SATS))
        } else {
            SATS
        };
        Ok(Some((amount, 0, payment_state)))
    }
}

impl Wallet {
    /// Where a bond stands: held while the trade runs, returned once it
    /// ends, settled only by a daemon that slashes honest takers.
    fn bond_fate(state: &State, invoice: &IssuedInvoice) -> PaymentState {
        let order = invoice.order.as_deref().expect("a bond names its order");
        if invoice.settled || state.slashed.get(order) == Some(&invoice.actor) {
            return PaymentState::Settled;
        }
        // A taker's bond is released with the take (a decline, a slash of
        // the other side); a maker's only when the order ends.
        let released = state.declined(invoice.actor, order)
            || state.order(order).is_terminal()
            || (state.maker() != Some(invoice.actor) && state.order(order) == OrderState::Pending);
        match (released, state.misbehaviour.slashes_honest_bonds) {
            (false, _) => PaymentState::Pending,
            (true, true) => PaymentState::Settled,
            (true, false) => PaymentState::Failed,
        }
    }

    /// Where a payment of a trade stands, from the node that made or
    /// received it.
    fn fate(observer: Actor, state: &State, invoice: &IssuedInvoice) -> PaymentState {
        match invoice.actor {
            // Alice's node paid the hold: settled once the daemon settled
            // the escrow at release, returned when the order was cancelled.
            Actor::Alice => {
                assert_eq!(observer, Actor::Alice);
                let order = invoice.order.as_deref().expect("hold names its order");
                if state.refunded {
                    PaymentState::Failed
                } else if invoice.settled {
                    PaymentState::Settled
                } else {
                    assert!(state.funded.contains(order));
                    match state.order(order) {
                        OrderState::Completed | OrderState::PayoutPending => PaymentState::Settled,
                        _ => PaymentState::Pending,
                    }
                }
            }
            // Bob's node issued the payout invoice.
            Actor::Bob => {
                assert_eq!(observer, Actor::Bob);
                if invoice.settled {
                    PaymentState::Settled
                } else if invoice.superseded {
                    PaymentState::Failed
                } else {
                    PaymentState::Pending
                }
            }
        }
    }
}
