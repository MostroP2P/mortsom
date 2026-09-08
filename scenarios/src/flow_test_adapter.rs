use super::*;
use mortsom_android_driver::process::CommandRunner;
use mortsom_app_contract::*;

pub struct Adapter {
    pub(super) state: Arc<Mutex<State>>,
    pub(super) relay: Arc<RelayObserver>,
    pub(super) keys: Keys,
}
impl Adapter {
    pub fn created(&self) -> (Actor, OrderKind) {
        self.state.lock().unwrap().created.unwrap()
    }
    async fn transition(&self, state: OrderState, wire: &str) {
        self.state.lock().unwrap().order = state;
        let event = EventBuilder::new(Kind::Custom(38383), "")
            .tags([
                Tag::parse(["d", ORDER]).unwrap(),
                Tag::parse(["s", wire]).unwrap(),
            ])
            .finalize(&self.keys)
            .unwrap();
        assert!(self.relay.ingest("ws://fixture.invalid", &event).is_some());
    }
    fn action(&self, verb: &str, actor: Actor) {
        self.state
            .lock()
            .unwrap()
            .actions
            .push(format!("{verb}:{actor}"));
    }
}
#[async_trait]
impl AppAdapter for Adapter {
    fn profile(&self) -> AppProfile {
        AppProfile {
            name: "fixture".into(),
            package: "fixture".into(),
        }
    }
    fn capabilities(&self) -> CapabilitySet {
        CapabilitySet::default()
    }
    async fn build(&self, _: &BuildContext, _: &dyn CommandRunner) -> Result<Apk, AdapterError> {
        unreachable!()
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
    async fn create_order(
        &self,
        device: &DeviceContext<'_>,
        draft: &OrderDraft,
    ) -> Result<VisibleOrder, AdapterError> {
        self.action("create", device.actor);
        self.state.lock().unwrap().created = Some((device.actor, draft.kind));
        self.transition(OrderState::Pending, "pending").await;
        Ok(VisibleOrder {
            id: Some(ORDER.into()),
            state: OrderState::Pending,
        })
    }
    async fn find_and_take_order(
        &self,
        device: &DeviceContext<'_>,
        order: &OrderReference,
    ) -> Result<(), AdapterError> {
        assert_eq!(order.kind, Some(OrderKind::Buy));
        self.action("take", device.actor);
        self.state.lock().unwrap().order = OrderState::Taken;
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
        assert!(self.state.lock().unwrap().hold_started);
        self.transition(OrderState::Active, "active").await;
        Ok(invoice.cloned())
    }
    async fn confirm_fiat_sent(
        &self,
        device: &DeviceContext<'_>,
        _: &OrderReference,
    ) -> Result<(), AdapterError> {
        self.action("fiat", device.actor);
        self.transition(OrderState::FiatSent, "fiat-sent").await;
        Ok(())
    }
    async fn release(
        &self,
        device: &DeviceContext<'_>,
        _: &OrderReference,
    ) -> Result<(), AdapterError> {
        self.action("release", device.actor);
        self.transition(OrderState::Completed, "success").await;
        Ok(())
    }
    async fn cancel_if_open(
        &self,
        _: &DeviceContext<'_>,
        _: &OrderReference,
    ) -> Result<CancelOutcome, AdapterError> {
        unreachable!()
    }
    async fn observe_order(
        &self,
        _: &DeviceContext<'_>,
        _: &OrderReference,
    ) -> Result<AppOrderState, AdapterError> {
        let state = self.state.lock().unwrap().order;
        Ok(AppOrderState {
            state,
            raw_label: state.to_string(),
        })
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
