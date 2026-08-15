//! Settings: node identity readout and relay configuration.

use mortsom_app_contract::{AdapterError, DeviceContext, TestEnvironment};

use super::{expect, label, present, tap, type_into};
use crate::selectors::Ids;

/// Verifies the selected node pubkey shown in settings equals the daemon.
pub async fn verify_node(
    device: &DeviceContext<'_>,
    env: &TestEnvironment,
) -> Result<(), AdapterError> {
    let shown = label(device, Ids::SETTINGS_MOSTRO_NODE_PUBKEY).await?;
    if !same_pubkey(&shown, &env.mostro_pubkey) {
        return Err(AdapterError::product(format!(
            "settings show node `{shown}` but the daemon under test is `{}`",
            env.mostro_pubkey
        )));
    }
    Ok(())
}

/// Ensures the test relay is configured: adds it through the relay dialog
/// unless the relay row already exists (seeded by the test build).
pub async fn ensure_relay(
    device: &DeviceContext<'_>,
    env: &TestEnvironment,
) -> Result<(), AdapterError> {
    let row = Ids::relay_item(&env.relay_url);
    if present(device, &row).await? {
        return Ok(());
    }
    tap(device, Ids::SETTINGS_RELAYS_ADD).await?;
    type_into(device, Ids::SETTINGS_RELAYS_ADD_URL, &env.relay_url).await?;
    tap(device, Ids::SETTINGS_RELAYS_ADD_CONFIRM).await?;
    expect(device, &row).await
}

/// Pubkeys compare case-insensitively; the app may show hex or npub, so an
/// npub in either side only matches an identical npub.
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
