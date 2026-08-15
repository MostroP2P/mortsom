//! NWC configuration through the normal settings UI.

use mortsom_app_contract::{AdapterError, DeviceContext, WalletConnection};
use mortsom_common::Secret;

use super::{label, tap, type_into};
use crate::selectors::Ids;

/// Label the app exposes on `wallet.connection` when connected.
pub const CONNECTED: &str = "connected";

/// From settings: opens wallet settings, then the connect screen, enters
/// the URI, connects and reads the connection state back from settings.
pub async fn connect(
    device: &DeviceContext<'_>,
    label_name: &str,
    uri: &Secret,
) -> Result<WalletConnection, AdapterError> {
    tap(device, Ids::SETTINGS_WALLET).await?;
    tap(device, Ids::WALLET_SETTINGS_CONNECT).await?;
    type_into(device, Ids::WALLET_NWC_URI, uri.expose()).await?;
    tap(device, Ids::WALLET_NWC_CONNECT).await?;
    // On success the app navigates away from the connect screen; the
    // settings card then reports the state.
    let state = wait_connection_state(device).await?;
    Ok(WalletConnection {
        label: label_name.to_owned(),
        connected: state == CONNECTED,
    })
}

/// Reads `wallet.connection`, waiting for it to be present.
pub async fn wait_connection_state(device: &DeviceContext<'_>) -> Result<String, AdapterError> {
    label(device, Ids::WALLET_CONNECTION).await
}
