//! NWC configuration: settings → wallet, URI typed, connect.

use mortsom_app_contract::{AdapterError, DeviceContext, WalletConnection};
use mortsom_common::Secret;

use super::{label, tap, type_into};
use crate::selectors::Ids;

/// Label of `wallet.connection` when connected.
pub const CONNECTED: &str = "connected";

/// Connects the wallet and reads the connection state back.
///
/// The wallet entry in settings opens the connect form while no wallet is
/// configured, which a fresh run always is. A configured wallet offers only
/// `wallet.settings.disconnect`; the app has no control to connect another
/// one over it.
pub async fn connect(
    device: &DeviceContext<'_>,
    label_name: &str,
    uri: &Secret,
) -> Result<WalletConnection, AdapterError> {
    tap(device, Ids::SETTINGS_WALLET).await?;
    type_into(device, Ids::WALLET_NWC_URI, uri.expose()).await?;
    tap(device, Ids::WALLET_NWC_CONNECT).await?;
    let state = label(device, Ids::WALLET_CONNECTION).await?;
    Ok(WalletConnection {
        label: label_name.to_owned(),
        connected: state == CONNECTED,
    })
}
