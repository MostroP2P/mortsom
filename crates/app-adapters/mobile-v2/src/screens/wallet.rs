//! NWC configuration: settings → wallet → connect, URI typed, connect.

use mortsom_app_contract::{AdapterError, DeviceContext, WalletConnection};
use mortsom_common::Secret;

use super::{label, present, tap, type_into};
use crate::selectors::Ids;

/// Label of `wallet.connection` when connected.
pub const CONNECTED: &str = "connected";

/// Connects the wallet and reads the connection state back.
///
/// The wallet entry in settings leads straight to the connect screen while
/// no wallet is configured, and to the wallet screen once one is — where
/// the connect button lives. Tapping it unconditionally would miss the
/// first case, which is the one a fresh run is always in.
pub async fn connect(
    device: &DeviceContext<'_>,
    label_name: &str,
    uri: &Secret,
) -> Result<WalletConnection, AdapterError> {
    tap(device, Ids::SETTINGS_WALLET).await?;
    if present(device, Ids::WALLET_SETTINGS_CONNECT).await? {
        tap(device, Ids::WALLET_SETTINGS_CONNECT).await?;
    }
    type_into(device, Ids::WALLET_NWC_URI, uri.expose()).await?;
    tap(device, Ids::WALLET_NWC_CONNECT).await?;
    let state = label(device, Ids::WALLET_CONNECTION).await?;
    Ok(WalletConnection {
        label: label_name.to_owned(),
        connected: state == CONNECTED,
    })
}
