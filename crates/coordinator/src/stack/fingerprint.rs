//! Deterministic identity of the public stack configuration used by a run.
use super::StackSnapshot;
use serde::{Deserialize, Serialize};

/// Versioned digest of the inspected public stack descriptor.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StackFingerprint {
    /// Digest framing and serialization version.
    pub algorithm: String,
    /// SHA-256 digest; excludes credential paths, secrets, timestamps and run IDs.
    pub digest: String,
}

impl StackSnapshot {
    /// Hashes daemon identity/image, relay/wallet endpoints and declared limits.
    pub fn fingerprint(&self) -> Result<StackFingerprint, String> {
        use sha2::{Digest, Sha256};
        let encoded =
            serde_json::to_vec(self).map_err(|_| "cannot encode public stack configuration")?;
        let mut hash = Sha256::new();
        hash.update(b"mortsom-stack-v1\0");
        hash.update((encoded.len() as u64).to_be_bytes());
        hash.update(encoded);
        Ok(StackFingerprint {
            algorithm: "sha256-mortsom-stack-v1".into(),
            digest: hash
                .finalize()
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect(),
        })
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn endpoint_map_order_is_irrelevant_but_changed_limits_or_identity_change_the_digest() {
        let mut snapshot: StackSnapshot = serde_json::from_value(serde_json::json!({
            "daemon_pubkey":"public", "relay_url":"ws://127.0.0.1:7000",
            "wallet_endpoints":{"bob":"https://127.0.0.1:10002","alice":"https://127.0.0.1:10001"},
            "daemon_image":{"image_id":"sha256:abc","reference":"image","repository_digest":null},
            "declared_limits":{"order_expiration_secs":300,"hold_invoice_expiry_secs":120,
                "buyer_invoice_deadline_secs":120,"payout_invoice_min_expiry_secs":300}
        }))
        .unwrap();
        let expected = snapshot.fingerprint().unwrap();
        let bob = snapshot.wallet_endpoints.remove("bob").unwrap();
        snapshot.wallet_endpoints.insert("bob".into(), bob);
        assert_eq!(expected, snapshot.fingerprint().unwrap());
        snapshot.declared_limits.order_expiration_secs += 1;
        assert_ne!(expected, snapshot.fingerprint().unwrap());
        snapshot.declared_limits.order_expiration_secs -= 1;
        snapshot.daemon_pubkey = "different".into();
        assert_ne!(expected, snapshot.fingerprint().unwrap());
    }
}
