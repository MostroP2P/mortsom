//! Identity and node: `identity init` is the CLI's first launch, `identity
//! show` its account screen, and a read of the public book proves the CLI
//! reaches the node under test through the run's relay.

use mortsom_app_contract::{AdapterError, DeviceContext, Identity, TestEnvironment};
use mortsom_cli_driver::{Call, Envelope};
use nostr::key::PublicKey;
use serde_json::Value;

use crate::access::{missing, run};

/// Generates the actor's identity in its fresh store.
pub(crate) async fn onboard(device: &DeviceContext<'_>) -> Result<Identity, AdapterError> {
    let outcome = run(device, Call::local(&["identity", "init"])).await?;
    identity_of(&outcome)
}

/// Reads the identity in the actor's store.
pub(crate) async fn displayed(device: &DeviceContext<'_>) -> Result<Identity, AdapterError> {
    let outcome = run(device, Call::local(&["identity", "show"])).await?;
    identity_of(&outcome)
}

fn identity_of(outcome: &Envelope) -> Result<Identity, AdapterError> {
    let key = outcome
        .text(&["identity_pubkey"])
        .ok_or_else(|| missing(&outcome.command, "identity_pubkey"))?;
    PublicKey::parse(key).map_err(|_| {
        AdapterError::contract(format!(
            "`{}` returned an identity that is not a public key",
            outcome.command
        ))
    })?;
    Ok(Identity {
        public_key: key.to_owned(),
    })
}

/// Reads the public book through the run's relay and checks that the CLI
/// answers for the daemon under test.
pub(crate) async fn check_node(
    device: &DeviceContext<'_>,
    env: &TestEnvironment,
) -> Result<(), AdapterError> {
    let outcome = run(device, Call::node(&["order", "list"])).await?;
    let node = outcome
        .text(&["node_pubkey"])
        .ok_or_else(|| missing("order.list", "node_pubkey"))?;
    if !same_key(node, &env.mostro_pubkey) {
        return Err(AdapterError::environment(format!(
            "the CLI reads the book of node {node}, not of the daemon under test"
        )));
    }
    if outcome
        .field(&["orders"])
        .and_then(Value::as_array)
        .is_none()
    {
        return Err(missing("order.list", "orders"));
    }
    Ok(())
}

fn same_key(a: &str, b: &str) -> bool {
    matches!((PublicKey::parse(a), PublicKey::parse(b)), (Ok(a), Ok(b)) if a == b)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use nostr::nips::nip19::ToBech32;

    #[test]
    fn keys_compare_whatever_their_encoding() {
        let hex = "dbe0b1be7aafd3cfba92d7463edbd4e33b2969f61bd554d37ac56f032e13355a";
        let npub = PublicKey::parse(hex).unwrap().to_bech32().unwrap();

        assert!(same_key(hex, &npub));
        assert!(!same_key(hex, "nope"));
    }
}
