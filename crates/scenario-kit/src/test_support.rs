//! Shared test fixtures: a state-only adapter, a UI session that does
//! nothing, a wallet reader with no payments and a context factory.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use mortsom_android_driver::appium::AppiumError;
use mortsom_android_driver::avd::Actor;
use mortsom_android_driver::ui::UiSession;
use mortsom_app_contract::{
    AdapterError, Apk, AppAdapter, AppOrderState, AppProfile, BuildContext, CancelOutcome,
    CapabilitySet, DeviceContext, Identity, OrderDraft, OrderReference, OrderState, PaymentHandoff,
    PaymentPath, TestEnvironment, VisibleOrder, WalletConnection,
};
use mortsom_common::{Lane, Secret};
use mortsom_observers::relay::{ObserverScope, RelayObserver};
use mortsom_observers::wallet::{WalletObserver, WalletReader};
use nostr_sdk::prelude::Keys;

use crate::budget::{Budget, SpendCaps};
use crate::cleanup::CleanupRegistry;
use crate::context::{ActorSession, Context};
use crate::evidence::NullSink;
use crate::recovery::{NoRelayControl, RelayControl};
use crate::timeline::Timeline;

/// Adapter whose only observation is a shared mutable state.
pub struct StateAdapter(pub Mutex<OrderState>);

#[async_trait]
impl AppAdapter for StateAdapter {
    fn profile(&self) -> AppProfile {
        AppProfile {
            name: "fake".into(),
            package: "p".into(),
        }
    }
    fn capabilities(&self) -> CapabilitySet {
        CapabilitySet::default()
    }
    async fn build(
        &self,
        _: &BuildContext,
        _: &dyn mortsom_android_driver::process::CommandRunner,
    ) -> Result<Apk, AdapterError> {
        Err(AdapterError::infrastructure("no"))
    }
    async fn onboard(
        &self,
        _: &DeviceContext<'_>,
        _: &TestEnvironment,
    ) -> Result<Identity, AdapterError> {
        Err(AdapterError::infrastructure("no"))
    }
    async fn displayed_identity(&self, _: &DeviceContext<'_>) -> Result<Identity, AdapterError> {
        Err(AdapterError::infrastructure("no"))
    }
    async fn configure_node_and_relay(
        &self,
        _: &DeviceContext<'_>,
        _: &TestEnvironment,
    ) -> Result<(), AdapterError> {
        Ok(())
    }
    async fn configure_nwc(
        &self,
        _: &DeviceContext<'_>,
        _: &str,
        _: &Secret,
    ) -> Result<WalletConnection, AdapterError> {
        Err(AdapterError::infrastructure("no"))
    }
    async fn owned_orders(&self, _: &DeviceContext<'_>) -> Result<Vec<String>, AdapterError> {
        Ok(vec![])
    }
    async fn create_order(
        &self,
        _: &DeviceContext<'_>,
        _: &OrderDraft,
    ) -> Result<VisibleOrder, AdapterError> {
        Err(AdapterError::infrastructure("no"))
    }
    async fn find_and_take_order(
        &self,
        _: &DeviceContext<'_>,
        _: &OrderReference,
    ) -> Result<(), AdapterError> {
        Ok(())
    }
    async fn pay_hold_invoice(
        &self,
        _: &DeviceContext<'_>,
        _: &OrderReference,
        _: PaymentPath,
    ) -> Result<PaymentHandoff, AdapterError> {
        Err(AdapterError::infrastructure("no"))
    }
    async fn provide_buyer_invoice(
        &self,
        _: &DeviceContext<'_>,
        _: &OrderReference,
        _: Option<&Secret>,
    ) -> Result<Option<Secret>, AdapterError> {
        Ok(None)
    }
    async fn confirm_fiat_sent(
        &self,
        _: &DeviceContext<'_>,
        _: &OrderReference,
    ) -> Result<(), AdapterError> {
        Ok(())
    }
    async fn release(&self, _: &DeviceContext<'_>, _: &OrderReference) -> Result<(), AdapterError> {
        Ok(())
    }
    async fn cancel_if_open(
        &self,
        _: &DeviceContext<'_>,
        _: &OrderReference,
    ) -> Result<CancelOutcome, AdapterError> {
        Ok(CancelOutcome::NotFound)
    }
    async fn visible_actions(
        &self,
        _: &DeviceContext<'_>,
        _: &OrderReference,
    ) -> Result<Vec<String>, AdapterError> {
        Ok(vec![])
    }
    async fn cancel_payment(
        &self,
        _: &DeviceContext<'_>,
        _: &OrderReference,
    ) -> Result<(), AdapterError> {
        Ok(())
    }
    async fn observe_order(
        &self,
        _: &DeviceContext<'_>,
        _: &OrderReference,
    ) -> Result<AppOrderState, AdapterError> {
        let state = *self.0.lock().unwrap();
        Ok(AppOrderState {
            state,
            raw_label: state.as_str().into(),
        })
    }
}

/// UI session that accepts every command and finds nothing.
pub struct NoUi;
#[async_trait]
impl UiSession for NoUi {
    async fn exists(&self, _: &str) -> Result<bool, AppiumError> {
        Ok(false)
    }
    async fn wait_for(&self, _: &str, _: Duration) -> Result<(), AppiumError> {
        Ok(())
    }
    async fn wait_gone(&self, _: &str, _: Duration) -> Result<(), AppiumError> {
        Ok(())
    }
    async fn tap(&self, _: &str) -> Result<(), AppiumError> {
        Ok(())
    }
    async fn type_text(&self, _: &str, _: &str) -> Result<(), AppiumError> {
        Ok(())
    }
    async fn text_of(&self, _: &str) -> Result<String, AppiumError> {
        Ok(String::new())
    }
    async fn is_enabled(&self, _: &str) -> Result<bool, AppiumError> {
        Ok(true)
    }
    async fn label_of(&self, _: &str) -> Result<String, AppiumError> {
        Ok(String::new())
    }
    async fn screenshot_png(&self) -> Result<Vec<u8>, AppiumError> {
        Ok(vec![])
    }
    async fn page_source(&self) -> Result<String, AppiumError> {
        Ok(String::new())
    }
    async fn background(&self, _: u64) -> Result<(), AppiumError> {
        Ok(())
    }
    async fn terminate_app(&self) -> Result<(), AppiumError> {
        Ok(())
    }
    async fn activate_app(&self) -> Result<(), AppiumError> {
        Ok(())
    }
    async fn clear_app(&self) -> Result<(), AppiumError> {
        Ok(())
    }
    async fn set_connectivity(&self, _: bool) -> Result<(), AppiumError> {
        Ok(())
    }
    async fn hide_keyboard(&self) -> Result<(), AppiumError> {
        Ok(())
    }
    async fn set_orientation(&self, _: bool) -> Result<(), AppiumError> {
        Ok(())
    }
    async fn end(&self) -> Result<(), AppiumError> {
        Ok(())
    }
}

/// Wallet reader with no payments.
pub struct NoWallet;
#[async_trait]
impl WalletReader for NoWallet {
    async fn lookup(
        &self,
        _: &str,
    ) -> Result<Option<(u64, u64, mortsom_observers::wallet::PaymentState)>, String> {
        Ok(None)
    }
}

/// A context over `adapter` with two silent actors and short deadlines.
pub fn context(adapter: Arc<dyn AppAdapter>) -> Context {
    context_with(adapter, Arc::new(NoRelayControl))
}

/// Same as [`context`] with an explicit relay control.
pub fn context_with(adapter: Arc<dyn AppAdapter>, relay_control: Arc<dyn RelayControl>) -> Context {
    Context {
        journal: Arc::new(crate::journal::NoJournal),
        node_wallets: std::collections::BTreeMap::new(),
        adapter,
        actors: vec![
            ActorSession {
                actor: Actor::Alice,
                ui: Arc::new(NoUi),
                wallet_label: "alice".into(),
            },
            ActorSession {
                actor: Actor::Bob,
                ui: Arc::new(NoUi),
                wallet_label: "bob".into(),
            },
        ],
        env: TestEnvironment {
            mostro_pubkey: "x".into(),
            relay_url: "ws://r".into(),
            lane: Lane::Local,
        },
        relay: Arc::new(RelayObserver::detached(ObserverScope::daemon_only(
            Keys::generate().public_key(),
        ))),
        relay_control,
        wallets: Arc::new(WalletObserver::new(vec![(
            "alice".into(),
            Arc::new(NoWallet) as Arc<dyn WalletReader>,
        )])),
        budget: Arc::new(Mutex::new(Budget::new(SpendCaps::new(10, 10).unwrap()))),
        cleanup: Arc::new(CleanupRegistry::default()),
        timeline: Arc::new(Timeline::default()),
        evidence: Arc::new(NullSink),
        pending_holds: Mutex::new(std::collections::BTreeMap::new()),
        mainnet: None,
        amount_sats: 5,
        fiat_code: "USD".into(),
        fiat_amount: 1,
        payment_method: "cash".into(),
        ui_timeout: Duration::from_millis(10),
        state_timeout: Duration::from_millis(300),
        settle_timeout: Duration::from_millis(10),
        expiration_timeout: Duration::from_millis(300),
        recovery: crate::context::RecoveryTiming {
            quiet_window: Duration::from_millis(20),
            background_secs: 1,
            outage: Duration::from_millis(5),
        },
    }
}
