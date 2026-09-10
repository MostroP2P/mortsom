use super::*;
use mortsom_app_contract::*;

/// How long the fixture daemon takes to reap an abandoned take or to
/// publish a remainder after settlement.
const DAEMON_DELAY: Duration = Duration::from_millis(30);

pub struct Adapter {
    pub(super) state: Arc<Mutex<State>>,
    pub(super) relay: Arc<RelayObserver>,
    pub(super) keys: Keys,
}
impl Adapter {
    pub fn created(&self) -> (Actor, OrderKind) {
        self.state.lock().unwrap().created.unwrap()
    }
    /// The app moves to `state`; the public book stays as it was. The live
    /// daemon keeps `active` and `fiat-sent` off the book: only pending,
    /// in-progress, canceled, success and expired are published.
    fn private_transition(&self, state: OrderState) {
        self.state.lock().unwrap().order = state;
    }
    async fn transition(&self, state: OrderState, wire: &str) {
        self.state.lock().unwrap().order = state;
        self.publish(ORDER, wire, &[]).await;
    }
    /// Publishes `order` as `wire` on the fixture relay with `extra` tags.
    /// An order outside the observer's scope is retried until the scenario
    /// scopes it, the way a relay replays a stored replaceable event to a
    /// late subscriber.
    async fn publish(&self, order: &str, wire: &str, extra: &[Vec<String>]) {
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
    /// The book tags of the created order: sats only once priced, fiat as
    /// one amount or both bounds.
    /// The daemon's dispute event: the dispute id, its status and who
    /// opened it.
    async fn publish_dispute(&self, status: &str) {
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
    fn book_tags(&self) -> Vec<Vec<String>> {
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
    fn action(&self, verb: &str, actor: Actor) {
        self.state
            .lock()
            .unwrap()
            .actions
            .push(format!("{verb}:{actor}"));
    }
    async fn take(&self, actor: Actor) {
        self.action("take", actor);
        // The live daemon republishes a taken order as `in-progress`.
        self.transition(OrderState::Taken, "in-progress").await;
        let reap = self.state.lock().unwrap().reaps_abandoned_take_to;
        if let Some(wire) = reap {
            let state = self.state.clone();
            let this = Adapter {
                state: state.clone(),
                relay: self.relay.clone(),
                keys: self.keys.clone(),
            };
            tokio::spawn(async move {
                tokio::time::sleep(DAEMON_DELAY).await;
                let reaped = OrderState::from_protocol_status(wire).unwrap();
                if reaped.is_terminal() {
                    // A take the daemon closed is dropped from both lists.
                    this.state.lock().unwrap().dropped.push(ORDER.to_owned());
                }
                this.transition(reaped, wire).await;
            });
        }
    }
}

fn uuid_like() -> String {
    format!("{:?}", std::time::Instant::now())
}

#[async_trait]
impl AppAdapter for Adapter {
    fn profile(&self) -> AppProfile {
        AppProfile {
            name: "fixture".into(),
        }
    }
    fn capabilities(&self) -> CapabilitySet {
        CapabilitySet::default()
    }
    async fn onboard(
        &self,
        _: &DeviceContext<'_>,
        _: &TestEnvironment,
    ) -> Result<Identity, AdapterError> {
        unreachable!()
    }
    async fn displayed_identity(&self, _: &DeviceContext<'_>) -> Result<Identity, AdapterError> {
        unreachable!()
    }
    async fn configure_node_and_relay(
        &self,
        _: &DeviceContext<'_>,
        _: &TestEnvironment,
    ) -> Result<(), AdapterError> {
        unreachable!()
    }
    async fn configure_nwc(
        &self,
        _: &DeviceContext<'_>,
        _: &str,
        _: &Secret,
    ) -> Result<WalletConnection, AdapterError> {
        unreachable!()
    }
    async fn owned_orders(&self, _: &DeviceContext<'_>) -> Result<Vec<String>, AdapterError> {
        let state = self.state.lock().unwrap();
        let mut ids = vec![];
        if state.created.is_some() && !state.dropped.iter().any(|id| id == ORDER) {
            ids.push(ORDER.to_owned());
        }
        if state.remainder.is_some() {
            ids.push(REMAINDER.to_owned());
        }
        Ok(ids)
    }
    async fn create_order(
        &self,
        device: &DeviceContext<'_>,
        draft: &OrderDraft,
    ) -> Result<VisibleOrder, AdapterError> {
        self.action("create", device.actor);
        {
            let mut state = self.state.lock().unwrap();
            state.created = Some((device.actor, draft.kind));
            state.draft = Some(draft.clone());
            state.order = OrderState::Pending;
        }
        let tags = self.book_tags();
        self.publish(ORDER, "pending", &tags).await;
        if self.state.lock().unwrap().expires_pending {
            let this = Adapter {
                state: self.state.clone(),
                relay: self.relay.clone(),
                keys: self.keys.clone(),
            };
            tokio::spawn(async move {
                tokio::time::sleep(DAEMON_DELAY).await;
                let untaken = this.state.lock().unwrap().order == OrderState::Pending;
                if untaken {
                    // The daemon's own status is `expired`; the book says
                    // `canceled`, as for every order closed without a trade.
                    this.state.lock().unwrap().order = OrderState::Expired;
                    this.publish(ORDER, "canceled", &[]).await;
                }
            });
        }
        Ok(VisibleOrder {
            id: Some(ORDER.into()),
            state: OrderState::Pending,
        })
    }
    async fn find_and_take_order(
        &self,
        device: &DeviceContext<'_>,
        _: &OrderReference,
    ) -> Result<(), AdapterError> {
        self.take(device.actor).await;
        Ok(())
    }
    async fn find_and_take_order_at(
        &self,
        device: &DeviceContext<'_>,
        _: &OrderReference,
        fiat_amount: u64,
    ) -> Result<(), AdapterError> {
        let is_range = self
            .state
            .lock()
            .unwrap()
            .draft
            .as_ref()
            .is_some_and(OrderDraft::is_range);
        if !is_range {
            return Err(AdapterError::contract(
                "the take amount dialog never appears for a fixed order",
            ));
        }
        self.action(&format!("take-at-{fiat_amount}"), device.actor);
        self.state.lock().unwrap().taken_at = Some(fiat_amount);
        self.take(device.actor).await;
        Ok(())
    }
    async fn pay_hold_invoice(
        &self,
        device: &DeviceContext<'_>,
        _: &OrderReference,
        path: PaymentPath,
    ) -> Result<PaymentHandoff, AdapterError> {
        self.action("hold", device.actor);
        assert_eq!(path, PaymentPath::ExternalWallet);
        if self.state.lock().unwrap().cancels_funded_take {
            // The buyer never invoices: after its window the daemon cancels
            // the order, returns the escrow, and both apps drop the trade.
            let this = Adapter {
                state: self.state.clone(),
                relay: self.relay.clone(),
                keys: self.keys.clone(),
            };
            tokio::spawn(async move {
                tokio::time::sleep(DAEMON_DELAY).await;
                let unpaid = {
                    let mut state = this.state.lock().unwrap();
                    if state.order != OrderState::Taken {
                        false
                    } else {
                        state.refunded = true;
                        state.dropped.push(ORDER.to_owned());
                        state.actions.push("daemon:cancel-unpaid".into());
                        true
                    }
                };
                if unpaid {
                    this.transition(OrderState::Cancelled, "canceled").await;
                }
            });
        }
        Ok(PaymentHandoff {
            kind: path,
            invoice: Some(Secret::new(INVOICE)),
        })
    }
    async fn buyer_invoice_amount(
        &self,
        _: &DeviceContext<'_>,
        _: &OrderReference,
    ) -> Result<u64, AdapterError> {
        Ok(250_000)
    }
    async fn provide_buyer_invoice(
        &self,
        device: &DeviceContext<'_>,
        _: &OrderReference,
        invoice: Option<&Secret>,
    ) -> Result<Option<Secret>, AdapterError> {
        self.action("invoice", device.actor);
        let kind = self.state.lock().unwrap().created.map(|(_, kind)| kind);
        if kind == Some(OrderKind::Buy) {
            assert!(self.state.lock().unwrap().hold_started);
            self.private_transition(OrderState::Active);
        }
        Ok(invoice.cloned())
    }
    async fn confirm_fiat_sent(
        &self,
        device: &DeviceContext<'_>,
        _: &OrderReference,
    ) -> Result<(), AdapterError> {
        self.action("fiat", device.actor);
        self.private_transition(OrderState::FiatSent);
        Ok(())
    }
    async fn release(
        &self,
        device: &DeviceContext<'_>,
        _: &OrderReference,
    ) -> Result<(), AdapterError> {
        self.action("release", device.actor);
        self.transition(OrderState::Completed, "success").await;
        // A release under dispute settles the dispute too.
        if self.state.lock().unwrap().dispute_initiator.is_some() {
            self.publish_dispute("settled").await;
            self.state.lock().unwrap().dispute_initiator = None;
        }
        let remainder = {
            let state = self.state.lock().unwrap();
            match (state.draft.as_ref().map(|d| d.fiat), state.taken_at) {
                (Some(FiatAmount::Range { max, .. }), Some(taken)) => Some(max - taken),
                _ => None,
            }
        };
        if let Some(left) = remainder {
            // The daemon spawns the remainder once the hold has settled.
            let this = Adapter {
                state: self.state.clone(),
                relay: self.relay.clone(),
                keys: self.keys.clone(),
            };
            tokio::spawn(async move {
                tokio::time::sleep(DAEMON_DELAY).await;
                this.state.lock().unwrap().remainder = Some(OrderState::Pending);
                this.publish(
                    REMAINDER,
                    "pending",
                    &[
                        vec!["amt".into(), "0".into()],
                        vec!["fa".into(), left.to_string()],
                    ],
                )
                .await;
            });
        }
        Ok(())
    }
    async fn cancel_if_open(
        &self,
        device: &DeviceContext<'_>,
        order: &OrderReference,
    ) -> Result<CancelOutcome, AdapterError> {
        self.action("cancel", device.actor);
        if order.id == REMAINDER {
            self.state.lock().unwrap().remainder = Some(OrderState::Cancelled);
            self.publish(REMAINDER, "canceled", &[]).await;
            return Ok(CancelOutcome::Cancelled);
        }
        let (state, lone_closes) = {
            let state = self.state.lock().unwrap();
            (state.order, state.misbehaviour.lone_cancel_closes)
        };
        match state {
            OrderState::Pending => {
                self.transition(OrderState::Cancelled, "canceled").await;
                Ok(CancelOutcome::Cancelled)
            }
            // A taker walking away puts the order back on the book.
            OrderState::Taken => {
                self.transition(OrderState::Pending, "pending").await;
                Ok(CancelOutcome::Cancelled)
            }
            OrderState::Active | OrderState::FiatSent => {
                let both = {
                    let mut guard = self.state.lock().unwrap();
                    if !guard.cancel_requests.contains(&device.actor) {
                        guard.cancel_requests.push(device.actor);
                    }
                    guard.cancel_requests.len() == Actor::ALL.len()
                };
                if both || lone_closes {
                    self.state.lock().unwrap().refunded = true;
                    self.transition(OrderState::Cancelled, "canceled").await;
                }
                Ok(CancelOutcome::Cancelled)
            }
            _ => Ok(CancelOutcome::AlreadyClosed),
        }
    }
    async fn open_dispute(
        &self,
        device: &DeviceContext<'_>,
        _: &OrderReference,
    ) -> Result<(), AdapterError> {
        self.action("dispute", device.actor);
        {
            let mut state = self.state.lock().unwrap();
            if !matches!(state.order, OrderState::Active | OrderState::FiatSent) {
                return Err(AdapterError::product(format!(
                    "no dispute control on a {} order",
                    state.order
                )));
            }
            state.order = OrderState::Disputed;
            let mut initiator = match device.actor {
                Actor::Alice => "seller",
                Actor::Bob => "buyer",
            };
            if state.misbehaviour.swaps_dispute_initiator {
                initiator = if initiator == "seller" {
                    "buyer"
                } else {
                    "seller"
                };
            }
            state.dispute_initiator = Some(initiator);
        }
        self.publish_dispute("initiated").await;
        Ok(())
    }
    async fn rate_counterpart(
        &self,
        device: &DeviceContext<'_>,
        _: &OrderReference,
        score: u8,
    ) -> Result<(), AdapterError> {
        assert_eq!(score, 5);
        self.action("rate", device.actor);
        self.state.lock().unwrap().rated.push(device.actor);
        let event = EventBuilder::new(
            Kind::Custom(mortsom_scenario_kit::actions::RATING_EVENT_KIND),
            "",
        )
        .tags([
            Tag::parse(["d", "counterpart-trade-key"]).unwrap(),
            Tag::parse(["nonce", &uuid_like()]).unwrap(),
        ])
        .finalize(&self.keys)
        .unwrap();
        assert!(self.relay.ingest("ws://fixture.invalid", &event).is_some());
        Ok(())
    }
    async fn observe_order(
        &self,
        device: &DeviceContext<'_>,
        order: &OrderReference,
    ) -> Result<AppOrderState, AdapterError> {
        let guard = self.state.lock().unwrap();
        if guard.dropped.contains(&order.id) {
            return Err(AdapterError::contract(format!(
                "`trades.item.{}` did not appear",
                order.id
            )));
        }
        if order.id == REMAINDER {
            let state = guard
                .remainder
                .ok_or_else(|| AdapterError::product("the app lists no remainder"))?;
            return Ok(AppOrderState {
                state,
                raw_label: state.to_string(),
            });
        }
        let state = guard.order;
        // The maker of a buy sees `waiting-payment` until the seller's hold
        // is accepted and `waiting-invoice` once the daemon asks for the
        // payout invoice; both are the coarse `taken`. A completed trade
        // asks this user for a rating until they give one.
        let raw_label = match state {
            OrderState::Taken if guard.hold_started => "waiting-invoice".to_owned(),
            OrderState::Taken => "waiting-payment".to_owned(),
            OrderState::Completed if guard.rated.contains(&device.actor) => "rated".to_owned(),
            OrderState::Completed => "pending-rating".to_owned(),
            other => other.to_string(),
        };
        Ok(AppOrderState { state, raw_label })
    }
    async fn visible_actions(
        &self,
        _: &DeviceContext<'_>,
        _: &OrderReference,
    ) -> Result<Vec<String>, AdapterError> {
        unreachable!()
    }
    async fn cancel_payment(
        &self,
        _: &DeviceContext<'_>,
        _: &OrderReference,
    ) -> Result<(), AdapterError> {
        unreachable!()
    }
}

/// The fixture solver: it rules the way the daemon reports a solver's
/// rulings, publishing the dispute status and moving the order.
#[async_trait]
impl mortsom_scenario_kit::context::DisputeSolver for Adapter {
    async fn take_dispute(&self, dispute_id: &str) -> Result<String, String> {
        assert_eq!(dispute_id, DISPUTE);
        self.state
            .lock()
            .unwrap()
            .actions
            .push("solver:take".into());
        self.publish_dispute("in-progress").await;
        let another = self
            .state
            .lock()
            .unwrap()
            .misbehaviour
            .solver_reports_another_order;
        Ok(if another {
            "another-order".to_owned()
        } else {
            ORDER.to_owned()
        })
    }
    async fn settle(&self, order_id: &str) -> Result<(), String> {
        assert_eq!(order_id, ORDER);
        self.state
            .lock()
            .unwrap()
            .actions
            .push("solver:settle".into());
        self.publish_dispute("settled").await;
        self.state.lock().unwrap().dispute_initiator = None;
        self.transition(OrderState::Completed, "success").await;
        Ok(())
    }
    async fn cancel(&self, order_id: &str) -> Result<(), String> {
        assert_eq!(order_id, ORDER);
        {
            let mut state = self.state.lock().unwrap();
            state.actions.push("solver:cancel".into());
            state.refunded = true;
        }
        self.publish_dispute("seller-refunded").await;
        self.state.lock().unwrap().dispute_initiator = None;
        self.transition(OrderState::Cancelled, "canceled").await;
        Ok(())
    }
}
