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

/// Returns to the order book from any drawer destination.
pub async fn go_home(device: &DeviceContext<'_>) -> Result<(), AdapterError> {
    tap(device, Ids::APPBAR_BACK).await?;
    expect(device, Ids::ORDER_ADD_FAB).await
}
