//! `AppAdapter` implementation for the classic app.

use std::path::PathBuf;

use async_trait::async_trait;
use mortsom_android_driver::process::CommandRunner;
use mortsom_app_contract::{
    AdapterError, Apk, AppAdapter, AppOrderState, AppProfile, BuildContext, CancelOutcome,
    Capability, CapabilitySet, DeviceContext, Identity, OrderDraft, OrderKind, OrderReference,
    PaymentHandoff, TestEnvironment, VisibleOrder, WalletConnection,
};
use mortsom_common::Secret;

use crate::build::build_apk;
use crate::screens::{self, label, nav, onboarding, orders, settings, wallet};
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
        device: &DeviceContext<'_>,
        draft: &OrderDraft,
    ) -> Result<VisibleOrder, AdapterError> {
        orders::create(device, draft).await
    }

    async fn find_and_take_order(
        &self,
        device: &DeviceContext<'_>,
        order: &OrderReference,
    ) -> Result<(), AdapterError> {
        // The book lists offers by the maker's side; a taker looks for the
        // opposite of what they want, so the reference carries the maker's kind.
        orders::find_and_take(device, order, order_kind_of(order)).await
    }

    async fn pay_hold_invoice(
        &self,
        device: &DeviceContext<'_>,
        order: &OrderReference,
    ) -> Result<PaymentHandoff, AdapterError> {
        orders::pay_hold_invoice(device, order).await
    }

    async fn provide_buyer_invoice(
        &self,
        device: &DeviceContext<'_>,
        order: &OrderReference,
    ) -> Result<Option<Secret>, AdapterError> {
        orders::provide_buyer_invoice(device, order).await
    }

    async fn confirm_fiat_sent(
        &self,
        device: &DeviceContext<'_>,
        order: &OrderReference,
    ) -> Result<(), AdapterError> {
        orders::confirm_fiat_sent(device, order).await
    }

    async fn release(
        &self,
        device: &DeviceContext<'_>,
        order: &OrderReference,
    ) -> Result<(), AdapterError> {
        orders::release(device, order).await
    }

    async fn cancel_if_open(
        &self,
        device: &DeviceContext<'_>,
        order: &OrderReference,
    ) -> Result<CancelOutcome, AdapterError> {
        orders::cancel_if_open(device, order).await
    }

    async fn observe_order(
        &self,
        device: &DeviceContext<'_>,
        order: &OrderReference,
    ) -> Result<AppOrderState, AdapterError> {
        orders::observe(device, order).await
    }
}

/// The maker's side recorded on the reference (`sell` unless stated).
fn order_kind_of(order: &OrderReference) -> OrderKind {
    order.kind.unwrap_or(OrderKind::Sell)
}

#[cfg(test)]
mod tests;
