//! Navigation: drawer destinations and back.

use mortsom_app_contract::{AdapterError, DeviceContext};

use super::{expect, tap};
use crate::selectors::Ids;

/// Opens the drawer and navigates to the account (key management) screen.
pub async fn go_to_account(device: &DeviceContext<'_>) -> Result<(), AdapterError> {
    tap(device, Ids::APPBAR_DRAWER).await?;
    tap(device, Ids::DRAWER_ACCOUNT).await?;
    expect(device, Ids::KEYS_PUBLIC_KEY).await
}

/// Opens the drawer and navigates to settings.
pub async fn go_to_settings(device: &DeviceContext<'_>) -> Result<(), AdapterError> {
    tap(device, Ids::APPBAR_DRAWER).await?;
    tap(device, Ids::DRAWER_SETTINGS).await?;
    expect(device, Ids::SETTINGS_MOSTRO_NODE).await
}

/// Returns to the order book from any drawer destination. A single back tap
/// is not enough on every build (some screens nest, others expose a bottom
/// bar instead of a back arrow), so this is the resilient unwind the order
/// screens already use.
pub async fn go_home(device: &DeviceContext<'_>) -> Result<(), AdapterError> {
    super::orders::go_home(device).await
}
