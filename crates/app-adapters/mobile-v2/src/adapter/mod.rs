//! `AppAdapter` implementation for the v2 app.

use std::path::PathBuf;

use async_trait::async_trait;
use mortsom_android_driver::process::CommandRunner;
use mortsom_app_contract::{
    AdapterError, Apk, AppAdapter, AppOrderState, AppProfile, BuildContext, CancelOutcome,
    Capability, CapabilitySet, DeviceContext, Identity, OrderDraft, OrderKind, OrderReference,
    PaymentHandoff, PaymentPath, TestEnvironment, VisibleOrder, WalletConnection,
};
use mortsom_common::Secret;

use crate::build::build_apk;
use crate::screens::{self, label, nav, onboarding, orders, settings, wallet};
use crate::selectors::Ids;

/// Profile name.
pub const PROFILE: &str = "mobile-v2";
/// Application id of the v2 app.
pub const DEFAULT_PACKAGE: &str = "foundation.mostro.app";

/// The v2 app adapter.
#[derive(Debug, Clone)]
pub struct MobileV2Adapter {
    package: String,
    flutter: PathBuf,
    relay_url_for_build: String,
}

impl MobileV2Adapter {
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
impl AppAdapter for MobileV2Adapter {
    fn profile(&self) -> AppProfile {
        AppProfile {
            name: PROFILE.into(),
            package: self.package.clone(),
        }
    }

    /// What the v2 contract covers (`docs/mobile-v2-inventory.md`).
    /// Notifications and deep links exist in v2 but are not part of the
    /// contract yet, so scenarios needing them are skipped with a reason.
    fn capabilities(&self) -> CapabilitySet {
        CapabilitySet::new([
            Capability::Onboarding,
            Capability::CustomNodeSelection,
            Capability::LocalRelayConfiguration,
            Capability::Nwc,
            Capability::CreateOrder,
            Capability::MarketPriceOrder,
            Capability::RangeOrder,
            Capability::TakeOrder,
            Capability::HoldInvoicePayment,
            Capability::ExternalWallet,
            Capability::BuyerInvoice,
            Capability::FiatSent,
            Capability::Release,
            Capability::CompletedObservation,
            Capability::Cancel,
            Capability::Dispute,
            Capability::Rating,
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

    /// Walkthrough, then the identity the app generated on first launch.
    async fn onboard(
        &self,
        device: &DeviceContext<'_>,
        _env: &TestEnvironment,
    ) -> Result<Identity, AdapterError> {
        onboarding::complete(device).await?;
        let identity = self.displayed_identity(device).await?;
        nav::go_home(device).await?;
        Ok(identity)
    }

    /// v2 generates the identity on first launch; an empty readout means
    /// the account screen has no key yet, so one is generated through the
    /// UI as a user would.
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
        settings::ensure_node(device, env).await?;
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

    async fn existing_identity(
        &self,
        device: &DeviceContext<'_>,
    ) -> Result<Identity, AdapterError> {
        nav::go_to_account(device).await?;
        let public_key = label(device, Ids::KEYS_PUBLIC_KEY).await?.trim().to_owned();
        if public_key.is_empty() {
            return Err(AdapterError::contract(
                "retained actor identity is missing; recovery cannot generate a replacement",
            ));
        }
        Ok(Identity { public_key })
    }
    async fn owned_orders(&self, device: &DeviceContext<'_>) -> Result<Vec<String>, AdapterError> {
        orders::trade_ids(device).await
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
        orders::find_and_take(device, order, order.kind.unwrap_or(OrderKind::Sell), None).await
    }

    async fn find_and_take_order_at(
        &self,
        device: &DeviceContext<'_>,
        order: &OrderReference,
        fiat_amount: u64,
    ) -> Result<(), AdapterError> {
        orders::find_and_take(
            device,
            order,
            order.kind.unwrap_or(OrderKind::Sell),
            Some(fiat_amount),
        )
        .await
    }

    async fn pay_hold_invoice(
        &self,
        device: &DeviceContext<'_>,
        order: &OrderReference,
        path: PaymentPath,
    ) -> Result<PaymentHandoff, AdapterError> {
        orders::pay_hold_invoice(device, order, path).await
    }

    async fn buyer_invoice_amount(
        &self,
        device: &DeviceContext<'_>,
        order: &OrderReference,
    ) -> Result<u64, AdapterError> {
        orders::buyer_invoice_amount(device, order).await
    }

    async fn provide_buyer_invoice(
        &self,
        device: &DeviceContext<'_>,
        order: &OrderReference,
        invoice: Option<&Secret>,
    ) -> Result<Option<Secret>, AdapterError> {
        orders::provide_buyer_invoice(device, order, invoice).await
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

    async fn visible_actions(
        &self,
        device: &DeviceContext<'_>,
        order: &OrderReference,
    ) -> Result<Vec<String>, AdapterError> {
        orders::visible_actions(device, order).await
    }

    async fn rate_counterpart(
        &self,
        device: &DeviceContext<'_>,
        order: &OrderReference,
        score: u8,
    ) -> Result<(), AdapterError> {
        orders::rate_counterpart(device, order, score).await
    }

    async fn open_dispute(
        &self,
        device: &DeviceContext<'_>,
        order: &OrderReference,
    ) -> Result<(), AdapterError> {
        orders::open_dispute(device, order).await
    }

    async fn cancel_payment(
        &self,
        device: &DeviceContext<'_>,
        order: &OrderReference,
    ) -> Result<(), AdapterError> {
        orders::cancel_payment(device, order).await
    }

    async fn observe_order(
        &self,
        device: &DeviceContext<'_>,
        order: &OrderReference,
    ) -> Result<AppOrderState, AdapterError> {
        orders::observe(device, order).await
    }
    async fn observe_order_detail(
        &self,
        device: &DeviceContext<'_>,
        order: &OrderReference,
    ) -> Result<AppOrderState, AdapterError> {
        orders::observe_detail(device, order).await
    }
}

#[cfg(test)]
mod tests;
