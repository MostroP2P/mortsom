//! Navigation: drawer destinations, bottom bar and back.

use mortsom_app_contract::{AdapterError, DeviceContext};

use super::{expect, present, tap};
use crate::selectors::Ids;

/// Deepest stack `go_home` unwinds (dialog over detail over list over book).
const MAX_BACK_STEPS: usize = 4;

/// Drawer → account (key management).
pub async fn go_to_account(device: &DeviceContext<'_>) -> Result<(), AdapterError> {
    go_home(device).await?;
    tap(device, Ids::APPBAR_DRAWER).await?;
    tap(device, Ids::DRAWER_ACCOUNT).await?;
    expect(device, Ids::KEYS_PUBLIC_KEY).await
}

/// Drawer → settings.
pub async fn go_to_settings(device: &DeviceContext<'_>) -> Result<(), AdapterError> {
    go_home(device).await?;
    tap(device, Ids::APPBAR_DRAWER).await?;
    tap(device, Ids::DRAWER_SETTINGS).await?;
    expect(device, Ids::SETTINGS_MOSTRO_NODE).await
}

/// Returns to the order book from anywhere reachable.
pub async fn go_home(device: &DeviceContext<'_>) -> Result<(), AdapterError> {
    for _ in 0..MAX_BACK_STEPS {
        if present(device, Ids::ORDER_ADD_FAB).await? {
            return Ok(());
        }
        if present(device, Ids::APPBAR_BACK).await? {
            tap(device, Ids::APPBAR_BACK).await?;
        } else if present(device, Ids::NAV_ORDER_BOOK).await? {
            tap(device, Ids::NAV_ORDER_BOOK).await?;
        } else {
            break;
        }
    }
    expect(device, Ids::ORDER_ADD_FAB).await.map_err(|e| {
        AdapterError::contract(format!(
            "could not return to the order book within {MAX_BACK_STEPS} back steps: {e}"
        ))
    })
}
