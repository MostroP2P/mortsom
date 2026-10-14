//! Navigation: drawer destinations, bottom bar and back.

use mortsom_app_contract::{AdapterError, DeviceContext};

use super::{expect, first_present, present, tap, tap_until};
use crate::selectors::Ids;

/// Deepest stack `go_home` unwinds (dialog over detail over list over book).
const MAX_BACK_STEPS: usize = 4;

/// Drawer → account (key management).
pub async fn go_to_account(device: &DeviceContext<'_>) -> Result<(), AdapterError> {
    go_home(device).await?;
    if !present(device, Ids::DRAWER_ACCOUNT).await? {
        tap_until(device, Ids::APPBAR_DRAWER, Ids::DRAWER_ACCOUNT).await?;
    }
    tap(device, Ids::DRAWER_ACCOUNT).await?;
    expect(device, Ids::KEYS_PUBLIC_KEY).await
}

/// Drawer → settings.
pub async fn go_to_settings(device: &DeviceContext<'_>) -> Result<(), AdapterError> {
    go_home(device).await?;
    if !present(device, Ids::DRAWER_SETTINGS).await? {
        tap_until(device, Ids::APPBAR_DRAWER, Ids::DRAWER_SETTINGS).await?;
    }
    tap(device, Ids::DRAWER_SETTINGS).await?;
    expect(device, Ids::SETTINGS_MOSTRO_NODE).await
}

/// Returns to the order book from anywhere reachable.
///
/// A screen still drawing -- a route mid-transition, as after a look cut off
/// halfway through a navigation -- shows none of the controls this goes by
/// for a moment, so it waits for one to arrive instead of giving up on the
/// first look. And a control seen a moment ago may leave with its screen
/// before the press lands, so a press that finds it gone looks again.
pub async fn go_home(device: &DeviceContext<'_>) -> Result<(), AdapterError> {
    for _ in 0..MAX_BACK_STEPS {
        let way = first_present(
            device,
            &[Ids::ORDER_ADD_FAB, Ids::NAV_ORDER_BOOK, Ids::APPBAR_BACK],
        )
        .await?;
        match way {
            Some(id) if id == Ids::ORDER_ADD_FAB => return Ok(()),
            Some(id) => press_seen(device, id).await?,
            None => break,
        }
        if super::appears_within(
            device,
            Ids::ORDER_ADD_FAB,
            std::time::Duration::from_secs(3),
        )
        .await?
        {
            return Ok(());
        }
    }
    expect(device, Ids::ORDER_ADD_FAB).await.map_err(|e| {
        AdapterError::contract(format!(
            "could not return to the order book within {MAX_BACK_STEPS} back steps: {e}"
        ))
    })
}

/// Presses `id`, just seen, without waiting for it again: a control that
/// left with its screen in between was not missed, the app moved on.
async fn press_seen(device: &DeviceContext<'_>, id: &str) -> Result<(), AdapterError> {
    match device.ui.tap(id).await {
        Ok(()) => Ok(()),
        Err(_) if !present(device, id).await? => Ok(()),
        Err(e) => Err(AdapterError::infrastructure(format!("tap `{id}`: {e}"))),
    }
}
