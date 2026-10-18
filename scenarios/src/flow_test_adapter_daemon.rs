//! The fixture daemon's own moves: what it publishes on the relay, how it
//! takes an order, and how it pays out.
use super::*;

impl Adapter {
    pub fn created(&self) -> (Actor, OrderKind) {
        self.state.lock().unwrap().created.unwrap()
    }
    pub(crate) fn detached(&self) -> Adapter {
        Adapter {
            state: self.state.clone(),
            relay: self.relay.clone(),
            keys: self.keys.clone(),
        }
    }
    /// An adapter over `state` publishing with the run's daemon keys, for
    /// the wallet side of the fixture, which has no adapter in hand.
    pub(crate) fn for_state(state: &Arc<Mutex<State>>) -> Adapter {
        let (relay, keys) = state
            .lock()
            .unwrap()
            .daemon
            .clone()
            .expect("the fixture daemon is set up");
        Adapter {
            state: state.clone(),
            relay,
            keys,
        }
    }
    /// The app moves `order` to `state`; the public book stays as it was.
    /// The live daemon keeps `active` and `fiat-sent` off the book: only
    /// pending, in-progress, canceled, success and expired are published.
    pub(crate) fn private_transition(&self, order: &str, state: OrderState) {
        self.state.lock().unwrap().set(order, state);
    }
    pub(crate) async fn transition(&self, order: &str, state: OrderState, wire: &str) {
        self.state.lock().unwrap().set(order, state);
        self.publish(order, wire, &[]).await;
    }
    /// Publishes `order` as `wire` on the fixture relay with `extra` tags.
    /// An order outside the observer's scope is retried until the scenario
    /// scopes it, the way a relay replays a stored replaceable event to a
    /// late subscriber.
    pub(crate) async fn publish(&self, order: &str, wire: &str, extra: &[Vec<String>]) {
        let mut tags = vec![
            Tag::parse(["d", order]).unwrap(),
            Tag::parse(["s", wire]).unwrap(),
            // Two revisions in one second must not share an event id.
            Tag::parse(["nonce", &uuid_like()]).unwrap(),
        ];
        for tag in extra {
            tags.push(Tag::parse(tag.iter().map(String::as_str)).unwrap());
        }
        let event = EventBuilder::new(Kind::Custom(38383), "")
            .tags(tags)
            .finalize(&self.keys)
            .unwrap();
        if self.relay.ingest("ws://fixture.invalid", &event).is_some() {
            return;
        }
        let relay = self.relay.clone();
        tokio::spawn(async move {
            for _ in 0..200 {
                tokio::time::sleep(Duration::from_millis(5)).await;
                if relay.ingest("ws://fixture.invalid", &event).is_some() {
                    return;
                }
            }
        });
    }
    /// The daemon's dispute event: the dispute id, its status and who
    /// opened it.
    pub(crate) async fn publish_dispute(&self, status: &str) {
        let initiator = self
            .state
            .lock()
            .unwrap()
            .dispute_initiator
            .expect("a dispute is open");
        let event = EventBuilder::new(
            Kind::Custom(mortsom_scenario_kit::actions::DISPUTE_EVENT_KIND),
            "",
        )
        .tags([
            Tag::parse(["d", DISPUTE]).unwrap(),
            Tag::parse(["s", status]).unwrap(),
            Tag::parse(["initiator", initiator]).unwrap(),
            Tag::parse(["nonce", &uuid_like()]).unwrap(),
        ])
        .finalize(&self.keys)
        .unwrap();
        assert!(self.relay.ingest("ws://fixture.invalid", &event).is_some());
    }
    /// The book tags of the created order: sats only once priced, fiat as
    /// one amount or both bounds.
    pub(crate) fn book_tags(&self) -> Vec<Vec<String>> {
        let state = self.state.lock().unwrap();
        let Some(draft) = state.draft.as_ref() else {
            return vec![];
        };
        let amt = match draft.price {
            OrderPrice::Fixed { amount_sats } => amount_sats,
            OrderPrice::Market { .. } if state.misbehaviour.prices_market_orders_upfront => 1000,
            OrderPrice::Market { .. } => 0,
        };
        let mut fa = vec!["fa".to_owned()];
        match draft.fiat {
            FiatAmount::Exact(amount) => fa.push(amount.to_string()),
            FiatAmount::Range { min, max } => {
                fa.push(min.to_string());
                fa.push(max.to_string());
            }
        }
        vec![vec!["amt".to_owned(), amt.to_string()], fa]
    }
    pub(crate) fn action(&self, verb: &str, actor: Actor) {
        self.state
            .lock()
            .unwrap()
            .actions
            .push(format!("{verb}:{actor}"));
    }
    pub(crate) async fn take(&self, actor: Actor, order: &str) {
        self.action("take", actor);
        let (bonded, publishes_early) = {
            let state = self.state.lock().unwrap();
            (
                state.bonded,
                state.misbehaviour.publishes_take_before_bond_lock,
            )
        };
        if bonded {
            // The taker is parked on the bond; the book keeps the order
            // pending until the bond locks (unless the daemon misbehaves).
            {
                let mut state = self.state.lock().unwrap();
                state.set(order, OrderState::Taken);
                state.bond_requested.insert(order.to_owned());
            }
            if publishes_early {
                self.publish(order, "in-progress", &[]).await;
            }
            self.lapse_unpaid_bond(order, true);
            return;
        }
        // The live daemon republishes a taken order as `in-progress`.
        self.transition(order, OrderState::Taken, "in-progress")
            .await;
        let reap = self.state.lock().unwrap().reaps_abandoned_take_to;
        if let Some(wire) = reap {
            let this = self.detached();
            let order = order.to_owned();
            tokio::spawn(async move {
                tokio::time::sleep(DAEMON_DELAY).await;
                let reaped = OrderState::from_protocol_status(wire).unwrap();
                if reaped.is_terminal() {
                    // A take the daemon closed is dropped from both lists.
                    this.state.lock().unwrap().dropped.push(order.clone());
                }
                this.transition(&order, reaped, wire).await;
            });
        }
    }

    /// The daemon's payout after the escrow settled: paid at once to an
    /// ordinary invoice, refused by a withdrawn one (retried, then a new
    /// invoice asked for), and accepted-and-held by a hold invoice --
    /// unless the payee already holds as many as the cap allows, in which
    /// case the payout waits its turn.
    pub(crate) async fn dispatch_payout(&self, order: &str) {
        let hash = self.state.lock().unwrap().buyer_invoice[order].clone();
        let (invoice, cap_reached, pays_withdrawn, ignores_cap) = {
            let state = self.state.lock().unwrap();
            (
                state.invoices[&hash].clone(),
                state.held_payouts() >= PAYOUT_CAP as usize,
                state.misbehaviour.pays_withdrawn_invoices,
                state.misbehaviour.ignores_payout_cap,
            )
        };
        if invoice.superseded && !pays_withdrawn {
            // First failure, the declared retries, then the request for a
            // replacement invoice.
            let this = self.detached();
            let order = order.to_owned();
            tokio::spawn(async move {
                tokio::time::sleep(DAEMON_DELAY).await;
                let mut state = this.state.lock().unwrap();
                state.actions.push("daemon:ask-invoice".into());
                state.asks_invoice.insert(order.clone());
                state.set(&order, OrderState::Taken);
            });
            return;
        }
        if invoice.hold.is_some() {
            let mut state = self.state.lock().unwrap();
            if cap_reached && !ignores_cap {
                state.actions.push("daemon:hold-payout".into());
                state.gated.push(order.to_owned());
                return;
            }
            state.invoice_mut(&hash).hold = Some(HoldInvoiceState::Accepted);
            return;
        }
        self.state.lock().unwrap().invoice_mut(&hash).settled = true;
        Self::payout_landed(&self.state, order).await;
    }

    /// The payout for `order` settled: the trade completes and, when a
    /// payout was waiting for a slot, it goes out now.
    pub(crate) async fn payout_landed(state: &Arc<Mutex<State>>, order: &str) {
        let next = {
            let mut guard = state.lock().unwrap();
            guard.set(order, OrderState::Completed);
            if guard.gated.is_empty() {
                None
            } else {
                Some(guard.gated.remove(0))
            }
        };
        let this = Adapter::for_state(state);
        this.publish(order, "success", &[]).await;
        if let Some(next) = next {
            Box::pin(this.dispatch_payout(&next)).await;
        }
    }
}

pub(super) fn uuid_like() -> String {
    format!("{:?}", std::time::Instant::now())
}
