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
    classify(&shown, &env.mostro_pubkey)
}

/// Compares the pubkey settings display with the configured one. Equal
/// values pass. Differing encodings (npub against hex) cannot be compared
/// here, so they are an automation-contract gap: the app must expose the
/// identity in the configured encoding. Anything else is the app pointing
/// at another node, which is a product failure.
fn classify(shown: &str, expected: &str) -> Result<(), AdapterError> {
    if same_pubkey(shown, expected) {
        return Ok(());
    }
    if is_npub(shown) != is_npub(expected) {
        return Err(AdapterError::contract(format!(
            "settings show the node identity as `{shown}` while the run \
             configures `{expected}`; the encodings differ, so the adapter \
             cannot tell whether it is the same node"
        )));
    }
    Err(AdapterError::product(format!(
        "settings show node `{shown}` but the daemon under test is `{expected}`"
    )))
}

/// Bech32-encoded public keys are recognised by their human-readable part.
fn is_npub(value: &str) -> bool {
    value.trim().to_ascii_lowercase().starts_with("npub1")
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
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn pubkey_comparison_ignores_case_and_whitespace() {
        assert!(same_pubkey(" ABC ", "abc"));
        assert!(!same_pubkey("abc", "abd"));
    }

    /// A settings screen that renders npub while the run configures hex is
    /// the app choosing an encoding, not the app pointing at another node.
    /// Blaming the product for it would misroute triage.
    #[test]
    fn a_different_encoding_is_not_the_same_failure_as_a_different_node() {
        assert_eq!(
            classify(NPUB, HEX).unwrap_err().category,
            mortsom_common::FailureCategory::AutomationContract
        );
        assert_eq!(
            classify(HEX, NPUB).unwrap_err().category,
            mortsom_common::FailureCategory::AutomationContract
        );
        assert_eq!(
            classify(OTHER_HEX, HEX).unwrap_err().category,
            mortsom_common::FailureCategory::Product
        );
        assert!(classify(HEX, HEX).is_ok());
    }

    const HEX: &str = "8171eb680049f62ce8747d203f57c3f71de6eea308af6f995363896a1e47cc51";
    const OTHER_HEX: &str = "b889ff5b1513b641e2a139f661a661364979c5beee91842f8f0ef42ab558e9d4";
    const NPUB: &str = "npub1s9c7ke9qynkyt8gay6g8a03u0hrhnw4rpzhk7v4xcufd2reru3gsjqsyx8";
}
