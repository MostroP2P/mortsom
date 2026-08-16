//! Settings: node selector dialog and the relays screen.

use mortsom_app_contract::{AdapterError, DeviceContext, TestEnvironment};

use super::{expect, label, present, tap, type_into};
use crate::selectors::Ids;

/// Selects the daemon under test in the node selector unless it is already
/// the shown node.
pub async fn ensure_node(
    device: &DeviceContext<'_>,
    env: &TestEnvironment,
) -> Result<(), AdapterError> {
    let shown = label(device, Ids::SETTINGS_MOSTRO_NODE_PUBKEY).await?;
    if same_pubkey(&shown, &env.mostro_pubkey) {
        return Ok(());
    }
    tap(device, Ids::SETTINGS_MOSTRO_NODE).await?;
    type_into(device, Ids::NODE_CUSTOM_PUBKEY, &env.mostro_pubkey).await?;
    tap(device, Ids::NODE_CUSTOM_CONFIRM).await?;
    let shown = label(device, Ids::SETTINGS_MOSTRO_NODE_PUBKEY).await?;
    if !same_pubkey(&shown, &env.mostro_pubkey) {
        return Err(AdapterError::product(format!(
            "settings show node `{shown}` after selecting `{}`",
            env.mostro_pubkey
        )));
    }
    Ok(())
}

/// Opens the relays screen and adds the run relay unless it is listed.
pub async fn ensure_relay(
    device: &DeviceContext<'_>,
    env: &TestEnvironment,
) -> Result<(), AdapterError> {
    tap(device, Ids::SETTINGS_RELAYS).await?;
    let row = Ids::relay_item(&env.relay_url);
    if present(device, &row).await? {
        return Ok(());
    }
    tap(device, Ids::SETTINGS_RELAYS_ADD).await?;
    type_into(device, Ids::SETTINGS_RELAYS_ADD_URL, &env.relay_url).await?;
    tap(device, Ids::SETTINGS_RELAYS_ADD_CONFIRM).await?;
    expect(device, &row).await
}

fn same_pubkey(shown: &str, expected: &str) -> bool {
    shown.trim().eq_ignore_ascii_case(expected.trim())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pubkey_comparison_ignores_case_and_whitespace() {
        assert!(same_pubkey(" ABC ", "abc"));
        assert!(!same_pubkey("abc", "abd"));
    }
}
