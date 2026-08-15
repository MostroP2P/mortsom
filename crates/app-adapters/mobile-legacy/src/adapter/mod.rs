//! `AppAdapter` implementation for the classic app.

use std::path::PathBuf;

use async_trait::async_trait;
use mortsom_android_driver::process::CommandRunner;
use mortsom_app_contract::{
    AdapterError, Apk, AppAdapter, AppOrderState, AppProfile, BuildContext, CancelOutcome,
    Capability, CapabilitySet, DeviceContext, Identity, OrderDraft, OrderReference, PaymentHandoff,
    TestEnvironment, VisibleOrder, WalletConnection,
};
use mortsom_common::Secret;

use crate::build::build_apk;
use crate::screens::{self, label, nav, onboarding, settings, wallet};
use crate::selectors::Ids;

/// Profile name.
pub const PROFILE: &str = "mobile-legacy";

/// The classic app adapter.
#[derive(Debug, Clone)]
pub struct MobileLegacyAdapter {
    package: String,
    flutter: PathBuf,
    relay_url_for_build: String,
}

impl MobileLegacyAdapter {
    /// Creates the adapter for `package`, building with `flutter` and
    /// seeding `relay_url_for_build` into test builds.
    pub fn new(
        package: impl Into<String>,
        flutter: PathBuf,
        relay_url_for_build: impl Into<String>,
    ) -> Self {
        Self {
            package: package.into(),
            flutter,
            relay_url_for_build: relay_url_for_build.into(),
        }
    }
}

#[async_trait]
impl AppAdapter for MobileLegacyAdapter {
    fn profile(&self) -> AppProfile {
        AppProfile {
            name: PROFILE.into(),
            package: self.package.clone(),
        }
    }

    fn capabilities(&self) -> CapabilitySet {
        CapabilitySet::new([
            Capability::Onboarding,
            Capability::CustomNodeSelection,
            Capability::LocalRelayConfiguration,
            Capability::Nwc,
            Capability::CreateOrder,
            Capability::TakeOrder,
            Capability::HoldInvoicePayment,
            Capability::BuyerInvoice,
            Capability::FiatSent,
            Capability::Release,
            Capability::CompletedObservation,
            Capability::Cancel,
            Capability::Dispute,
            Capability::Rating,
            Capability::Notifications,
            Capability::DeepLinks,
            Capability::TestEnvironmentMarker,
        ])
    }

    async fn build(
        &self,
        ctx: &BuildContext,
        runner: &dyn CommandRunner,
    ) -> Result<Apk, AdapterError> {
        build_apk(
            runner,
            &self.flutter,
            ctx,
            &self.relay_url_for_build,
            &self.package,
        )
        .await
    }

    async fn onboard(
        &self,
        device: &DeviceContext<'_>,
        env: &TestEnvironment,
    ) -> Result<Identity, AdapterError> {
        onboarding::complete(device, env).await?;
        let identity = self.displayed_identity(device).await?;
        nav::go_home(device).await?;
        Ok(identity)
    }

    /// The app generates a master key on first launch; if the account
    /// screen still shows none, the adapter generates one through the UI so
    /// every actor ends with a fresh identity.
    async fn displayed_identity(
        &self,
        device: &DeviceContext<'_>,
    ) -> Result<Identity, AdapterError> {
        nav::go_to_account(device).await?;
        let mut public_key = label(device, Ids::KEYS_PUBLIC_KEY).await?;
        if public_key.trim().is_empty() {
            screens::tap(device, Ids::KEYS_GENERATE).await?;
            screens::tap(device, Ids::KEYS_GENERATE_CONFIRM).await?;
            public_key = label(device, Ids::KEYS_PUBLIC_KEY).await?;
        }
        if public_key.trim().is_empty() {
            return Err(AdapterError::product(
                "account screen shows an empty public key after generating one",
            ));
        }
        Ok(Identity {
            public_key: public_key.trim().to_owned(),
        })
    }

    async fn configure_node_and_relay(
        &self,
        device: &DeviceContext<'_>,
        env: &TestEnvironment,
    ) -> Result<(), AdapterError> {
        nav::go_to_settings(device).await?;
        settings::verify_node(device, env).await?;
        settings::ensure_relay(device, env).await?;
        nav::go_home(device).await
    }

    async fn configure_nwc(
        &self,
        device: &DeviceContext<'_>,
        label_name: &str,
        uri: &Secret,
    ) -> Result<WalletConnection, AdapterError> {
        nav::go_to_settings(device).await?;
        let connection = wallet::connect(device, label_name, uri).await?;
        nav::go_home(device).await?;
        Ok(connection)
    }

    async fn create_order(
        &self,
        _device: &DeviceContext<'_>,
        _draft: &OrderDraft,
    ) -> Result<VisibleOrder, AdapterError> {
        Err(not_yet("create_order"))
    }

    async fn find_and_take_order(
        &self,
        _device: &DeviceContext<'_>,
        _order: &OrderReference,
    ) -> Result<(), AdapterError> {
        Err(not_yet("find_and_take_order"))
    }

    async fn pay_hold_invoice(
        &self,
        _device: &DeviceContext<'_>,
        _order: &OrderReference,
    ) -> Result<PaymentHandoff, AdapterError> {
        Err(not_yet("pay_hold_invoice"))
    }

    async fn provide_buyer_invoice(
        &self,
        _device: &DeviceContext<'_>,
        _order: &OrderReference,
    ) -> Result<(), AdapterError> {
        Err(not_yet("provide_buyer_invoice"))
    }

    async fn confirm_fiat_sent(
        &self,
        _device: &DeviceContext<'_>,
        _order: &OrderReference,
    ) -> Result<(), AdapterError> {
        Err(not_yet("confirm_fiat_sent"))
    }

    async fn release(
        &self,
        _device: &DeviceContext<'_>,
        _order: &OrderReference,
    ) -> Result<(), AdapterError> {
        Err(not_yet("release"))
    }

    async fn cancel_if_open(
        &self,
        _device: &DeviceContext<'_>,
        _order: &OrderReference,
    ) -> Result<CancelOutcome, AdapterError> {
        Err(not_yet("cancel_if_open"))
    }

    async fn observe_order(
        &self,
        _device: &DeviceContext<'_>,
        _order: &OrderReference,
    ) -> Result<AppOrderState, AdapterError> {
        Err(not_yet("observe_order"))
    }
}

/// Order journeys arrive with the first end-to-end trade (spec Phase 4).
fn not_yet(action: &str) -> AdapterError {
    AdapterError::contract(format!(
        "{action} is not implemented for {PROFILE} yet (spec Phase 4)"
    ))
}

#[cfg(test)]
mod tests;
