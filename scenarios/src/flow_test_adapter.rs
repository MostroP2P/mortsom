use super::*;
use daemon::uuid_like;
use mortsom_app_contract::*;
use mortsom_observers::wallet::expiry_secs_of;

/// How long the fixture daemon takes to reap an abandoned take, to publish
/// a remainder after settlement, or to give up on a payout and ask for a
/// new invoice.
pub(crate) const DAEMON_DELAY: Duration = Duration::from_millis(30);
/// How long a waiting step or an unpaid bond lasts on the fixture daemon:
/// past the scenario's own looks right after the step is handed out, well
/// inside the expiration timeout.
pub(crate) const DEADLINE: Duration = Duration::from_millis(120);

pub struct Adapter {
    pub(super) state: Arc<Mutex<State>>,
    pub(super) relay: Arc<RelayObserver>,
    pub(super) keys: Keys,
}
#[path = "flow_test_adapter_daemon.rs"]
mod daemon;

#[async_trait]
impl AppAdapter for Adapter {
    fn profile(&self) -> AppProfile {
        AppProfile {
            name: "fixture".into(),
        }
    }
    /// Only what a scenario body asks for: whether the app judges invoices.
    fn capabilities(&self) -> CapabilitySet {
        if self.state.lock().unwrap().misbehaviour.lacks_invoice_check {
            CapabilitySet::default()
        } else {
            CapabilitySet::new([Capability::InvoiceCheck])
        }
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
    async fn owned_orders(&self, device: &DeviceContext<'_>) -> Result<Vec<String>, AdapterError> {
        let state = self.state.lock().unwrap();
        let mut ids: Vec<String> = state
            .orders
            .keys()
            .filter(|id| !state.dropped.contains(id) && !state.declined(device.actor, id))
            .cloned()
            .collect();
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
        let (id, maker_bond) = {
            let mut state = self.state.lock().unwrap();
            let id = State::order_name(state.orders.len());
            state.created = Some((device.actor, draft.kind));
            state.draft = Some(draft.clone());
            state.set(&id, OrderState::Pending);
            let maker_bond = state.bonded && BOND_POLICY.maker_posts();
            if maker_bond {
                // Parked on the maker's bond: nothing is published yet.
                state.maker_bond_requested.insert(id.clone());
            }
            (id, maker_bond)
        };
        if maker_bond {
            self.lapse_unpaid_bond(&id, false);
            return Ok(VisibleOrder {
                id: Some(id),
                state: OrderState::Taken,
            });
        }
        let tags = self.book_tags();
        self.publish(&id, "pending", &tags).await;
        if self.state.lock().unwrap().expires_pending {
            let this = self.detached();
            let order = id.clone();
            tokio::spawn(async move {
                tokio::time::sleep(DAEMON_DELAY).await;
                let untaken = this.state.lock().unwrap().order(&order) == OrderState::Pending;
                if untaken {
                    // The daemon's own status is `expired`; the book says
                    // `canceled`, as for every order closed without a trade.
                    this.state.lock().unwrap().set(&order, OrderState::Expired);
                    this.publish(&order, "canceled", &[]).await;
                }
            });
        }
        Ok(VisibleOrder {
            id: Some(id),
            state: OrderState::Pending,
        })
    }
    async fn find_and_take_order(
        &self,
        device: &DeviceContext<'_>,
        order: &OrderReference,
    ) -> Result<(), AdapterError> {
        self.take(device.actor, &order.id).await;
        Ok(())
    }
    async fn find_and_take_order_at(
        &self,
        device: &DeviceContext<'_>,
        order: &OrderReference,
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
        self.take(device.actor, &order.id).await;
        Ok(())
    }
    async fn pay_hold_invoice(
        &self,
        device: &DeviceContext<'_>,
        order: &OrderReference,
        path: PaymentPath,
    ) -> Result<PaymentHandoff, AdapterError> {
        self.action("hold", device.actor);
        assert_eq!(path, PaymentPath::ExternalWallet);
        // The daemon's hold invoice for this order, issued on its own node
        // and paid from Alice's.
        let invoice = {
            let mut state = self.state.lock().unwrap();
            state.issued += 1;
            let invoice = Secret::new(build_invoice(SATS * 1000, 3600, state.issued));
            let hash = payment_hash_of(&invoice).unwrap();
            state.invoices.insert(
                hash,
                IssuedInvoice {
                    actor: Actor::Alice,
                    order: Some(order.id.clone()),
                    hold: None,
                    superseded: false,
                    settled: false,
                    bond: false,
                    claim: false,
                },
            );
            invoice
        };
        if self.state.lock().unwrap().cancels_funded_take {
            // The buyer never invoices: after its window the daemon cancels
            // the order, returns the escrow, and both apps drop the trade.
            let this = self.detached();
            let id = order.id.clone();
            tokio::spawn(async move {
                tokio::time::sleep(DAEMON_DELAY).await;
                let unpaid = {
                    let mut state = this.state.lock().unwrap();
                    if state.order(&id) != OrderState::Taken {
                        false
                    } else {
                        state.refunded = true;
                        state.dropped.push(id.clone());
                        state.actions.push("daemon:cancel-unpaid".into());
                        true
                    }
                };
                if unpaid {
                    this.transition(&id, OrderState::Cancelled, "canceled")
                        .await;
                }
            });
        }
        Ok(PaymentHandoff {
            kind: path,
            invoice: Some(invoice),
        })
    }
    async fn pay_bond_invoice(
        &self,
        device: &DeviceContext<'_>,
        order: &OrderReference,
        path: PaymentPath,
    ) -> Result<PaymentHandoff, AdapterError> {
        self.action("bond", device.actor);
        assert_eq!(path, PaymentPath::ExternalWallet);
        let mut state = self.state.lock().unwrap();
        let maker = state.maker() == Some(device.actor);
        let offered = if maker {
            state.maker_bond_requested.contains(&order.id)
        } else {
            state.bond_requested.contains(&order.id)
        };
        if !offered {
            return Err(AdapterError::contract(format!(
                "no deposit action is offered to {} for order {}",
                device.actor, order.id
            )));
        }
        // The daemon's bond invoice for this order, sized by its policy
        // and paid from the payer's node.
        let sats = BOND_POLICY.bond_sats(SATS);
        state.issued += 1;
        let invoice = Secret::new(build_invoice(sats * 1000, 3600, state.issued));
        let hash = payment_hash_of(&invoice).unwrap();
        state.invoices.insert(
            hash,
            IssuedInvoice {
                actor: device.actor,
                order: Some(order.id.clone()),
                hold: None,
                superseded: false,
                settled: false,
                bond: true,
                claim: false,
            },
        );
        Ok(PaymentHandoff {
            kind: path,
            invoice: Some(invoice),
        })
    }
    async fn decline_bond(
        &self,
        device: &DeviceContext<'_>,
        order: &OrderReference,
    ) -> Result<(), AdapterError> {
        self.action("decline-bond", device.actor);
        let mut state = self.state.lock().unwrap();
        if state.maker() == Some(device.actor) {
            // The maker abandons an order never published: the app drops it.
            if !state.maker_bond_requested.remove(&order.id) {
                return Err(AdapterError::contract(format!(
                    "`bond.cancel` is not offered for order {}",
                    order.id
                )));
            }
            state.declined.push((device.actor, order.id.clone()));
            return Ok(());
        }
        if !state.bond_requested.remove(&order.id) {
            return Err(AdapterError::contract(format!(
                "`bond.cancel` is not offered for order {}",
                order.id
            )));
        }
        // The daemon releases only this taker's bond; the order was never
        // off the book.
        state.set(&order.id, OrderState::Pending);
        state.declined.push((device.actor, order.id.clone()));
        Ok(())
    }
    async fn bond_claim(
        &self,
        device: &DeviceContext<'_>,
        order: &OrderReference,
    ) -> Result<Option<BondClaim>, AdapterError> {
        self.claim_for(device.actor, &order.id)
    }
    async fn submit_bond_claim_invoice(
        &self,
        device: &DeviceContext<'_>,
        order: &OrderReference,
        invoice: &Secret,
    ) -> Result<String, AdapterError> {
        self.take_claim_invoice(device.actor, &order.id, invoice)
    }
    async fn bond_slash_notice(
        &self,
        device: &DeviceContext<'_>,
        order: &OrderReference,
    ) -> Result<Option<String>, AdapterError> {
        Ok(self.slash_notice_for(device.actor, &order.id))
    }
    async fn buyer_invoice_amount(
        &self,
        _: &DeviceContext<'_>,
        _: &OrderReference,
    ) -> Result<u64, AdapterError> {
        Ok(SATS)
    }
    async fn provide_buyer_invoice(
        &self,
        device: &DeviceContext<'_>,
        order: &OrderReference,
        invoice: Option<&Secret>,
    ) -> Result<Option<Secret>, AdapterError> {
        self.action("invoice", device.actor);
        let hash = payment_hash_of(invoice.expect("the fixture issues every invoice")).unwrap();
        let (kind, replacement) = {
            let mut state = self.state.lock().unwrap();
            state.invoice_mut(&hash).order = Some(order.id.clone());
            state.buyer_invoice.insert(order.id.clone(), hash);
            (
                state.created.map(|(_, kind)| kind),
                state.asks_invoice.remove(&order.id),
            )
        };
        if replacement {
            // The daemon takes the replacement and pays it.
            self.private_transition(&order.id, OrderState::PayoutPending);
            self.dispatch_payout(&order.id).await;
        } else if kind == Some(OrderKind::Buy) {
            assert!(self.state.lock().unwrap().funded.contains(&order.id));
            self.private_transition(&order.id, OrderState::Active);
        } else {
            // A sell's seller funds next, against the clock.
            Self::start_clock(&self.state, &order.id, Duty::Escrow);
        }
        Ok(invoice.cloned())
    }
    async fn submit_buyer_invoice(
        &self,
        device: &DeviceContext<'_>,
        order: &OrderReference,
        invoice: &Secret,
    ) -> Result<InvoiceSubmission, AdapterError> {
        let short = expiry_secs_of(invoice).unwrap() < EXPIRY_FLOOR_SECS;
        let odd = self.state.lock().unwrap().misbehaviour;
        // The app judges an invoice first once the node's capabilities
        // are in; one it refuses is never sent.
        if short && !odd.capabilities_arrive_late && !odd.lacks_invoice_check {
            self.action("invoice-refused-by-app", device.actor);
            let check = if odd.refuses_for_another_rule {
                "wrong-amount"
            } else {
                "expires-too-soon"
            };
            return Ok(InvoiceSubmission::RefusedByApp {
                check: check.into(),
            });
        }
        if short && !odd.accepts_short_invoices {
            self.action("invoice-refused", device.actor);
            return Ok(InvoiceSubmission::Rejected {
                reason: "The node rejected this invoice.".into(),
            });
        }
        self.provide_buyer_invoice(device, order, Some(invoice))
            .await?;
        let judged = !short && !odd.never_judges_valid && !odd.lacks_invoice_check;
        Ok(InvoiceSubmission::Accepted {
            check: judged.then(|| "valid".into()),
        })
    }
    async fn confirm_fiat_sent(
        &self,
        device: &DeviceContext<'_>,
        order: &OrderReference,
    ) -> Result<(), AdapterError> {
        self.action("fiat", device.actor);
        self.private_transition(&order.id, OrderState::FiatSent);
        Ok(())
    }
    async fn release(
        &self,
        device: &DeviceContext<'_>,
        order: &OrderReference,
    ) -> Result<(), AdapterError> {
        self.action("release", device.actor);
        // The escrow settles at once; the payout is the daemon's next
        // step, and how it goes depends on the buyer's invoice. A settled
        // escrow never unsettles, even while the order waits again for a
        // replacement invoice.
        self.private_transition(&order.id, OrderState::PayoutPending);
        {
            let mut state = self.state.lock().unwrap();
            let escrow = state.invoices.values_mut().find(|invoice| {
                invoice.actor == Actor::Alice
                    && !invoice.bond
                    && !invoice.claim
                    && invoice.order.as_deref() == Some(order.id.as_str())
            });
            if let Some(invoice) = escrow {
                invoice.settled = true;
            }
        }
        self.dispatch_payout(&order.id).await;
        // A release under dispute closes the dispute as released, as
        // mostrod does since mostro-core 0.17 (`Status::Released`).
        if self.state.lock().unwrap().dispute_initiator.is_some() {
            self.publish_dispute(mortsom_scenario_kit::actions::dispute_status::RELEASED)
                .await;
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
            let this = self.detached();
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
        // App v2 drops a trade that never went active from My Trades once
        // it is cancelled; it never shows it cancelled.
        let silent = self
            .state
            .lock()
            .unwrap()
            .misbehaviour
            .never_publishes_cancel;
        if order.id == REMAINDER {
            self.state.lock().unwrap().remainder = None;
            if !silent {
                self.publish(REMAINDER, "canceled", &[]).await;
            }
            return Ok(CancelOutcome::Cancelled);
        }
        let (state, lone_closes) = {
            let state = self.state.lock().unwrap();
            (
                state.order(&order.id),
                state.misbehaviour.lone_cancel_closes,
            )
        };
        match state {
            OrderState::Pending => {
                self.state.lock().unwrap().dropped.push(order.id.clone());
                if !silent {
                    self.transition(&order.id, OrderState::Cancelled, "canceled")
                        .await;
                }
                Ok(CancelOutcome::Cancelled)
            }
            // A taker walking away puts the order back on the book.
            OrderState::Taken => {
                self.transition(&order.id, OrderState::Pending, "pending")
                    .await;
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
                    self.transition(&order.id, OrderState::Cancelled, "canceled")
                        .await;
                }
                Ok(CancelOutcome::Cancelled)
            }
            _ => Ok(CancelOutcome::AlreadyClosed),
        }
    }
    async fn open_dispute(
        &self,
        device: &DeviceContext<'_>,
        order: &OrderReference,
    ) -> Result<(), AdapterError> {
        self.action("dispute", device.actor);
        {
            let mut state = self.state.lock().unwrap();
            let current = state.order(&order.id);
            if !matches!(current, OrderState::Active | OrderState::FiatSent) {
                return Err(AdapterError::product(format!(
                    "no dispute control on a {current} order"
                )));
            }
            state.set(&order.id, OrderState::Disputed);
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
        if guard.dropped.contains(&order.id) || guard.declined(device.actor, &order.id) {
            return Err(AdapterError::contract(format!(
                "`trades.item.{}` did not appear",
                order.id
            )));
        }
        // An order parked on its maker's bond is the maker's alone, and
        // reads as their bond window.
        if guard.maker_bond_requested.contains(&order.id) {
            return Ok(AppOrderState {
                state: OrderState::Taken,
                raw_label: "waiting-bond".into(),
            });
        }
        // While the taker's bond is outstanding only the taker knows the
        // order is taken: the maker's app and the book still show it
        // pending.
        if guard.bond_requested.contains(&order.id) {
            let (state, raw_label) = if guard.taker() == Some(device.actor) {
                (OrderState::Taken, "waiting-bond")
            } else {
                (OrderState::Pending, "pending")
            };
            return Ok(AppOrderState {
                state,
                raw_label: raw_label.into(),
            });
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
        let state = guard.order(&order.id);
        // The maker of a buy sees `waiting-payment` until the seller's hold
        // is accepted and `waiting-invoice` once the daemon asks for the
        // payout invoice; both are the coarse `taken`, as is the request
        // for a replacement after a failed payout. A completed trade asks
        // this user for a rating until they give one.
        let awaiting_invoice = guard.asks_invoice.contains(&order.id)
            || match guard.created.map(|(_, kind)| kind) {
                // The taker of a sell is asked for the invoice at the take.
                Some(OrderKind::Sell) => !guard.buyer_invoice.contains_key(&order.id),
                // The maker of a buy only once the seller's hold is accepted.
                Some(OrderKind::Buy) => {
                    guard.funded.contains(&order.id) && !guard.buyer_invoice.contains_key(&order.id)
                }
                None => false,
            };
        let raw_label = match state {
            OrderState::Taken if awaiting_invoice => "waiting-invoice".to_owned(),
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

#[path = "flow_test_adapter_bond.rs"]
mod bond;
#[path = "flow_test_adapter_solver.rs"]
mod solver;
