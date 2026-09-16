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
    /// Compare in the recorded schema: a v1-to-v2 upgrade alone is never
    /// evidence of a different stack. Unknown algorithms fail closed.
    pub fn matches_recorded(&self, recorded: &StackFingerprint) -> Result<bool, String> {
        let current = match recorded.algorithm.as_str() {
            "sha256-mortsom-stack-v2" => self.fingerprint()?,
            "sha256-mortsom-stack-v1" => self.legacy_fingerprint()?,
            _ => return Err("unsupported recovery stack fingerprint algorithm".into()),
        };
        Ok(current == *recorded)
    }

    pub(crate) fn legacy_fingerprint(&self) -> Result<StackFingerprint, String> {
        // Field order and framing are exactly the v1 wire representation.
        #[derive(Serialize)]
        struct Legacy<'a> {
            daemon_pubkey: &'a str,
            relay_url: &'a str,
            wallet_endpoints: &'a std::collections::BTreeMap<String, String>,
            daemon_image: &'a super::DaemonImage,
            declared_limits: &'a super::DeclaredLimits,
        }
        let encoded = serde_json::to_vec(&Legacy {
            daemon_pubkey: &self.daemon_pubkey,
            relay_url: &self.relay_url,
            wallet_endpoints: &self.wallet_endpoints,
            daemon_image: &self.daemon_image,
            declared_limits: &self.declared_limits,
        })
        .map_err(|_| "cannot encode legacy public stack configuration")?;
        Ok(fingerprint(&encoded, "v1"))
    }

    /// Hashes daemon identity/image/source, relay/wallet endpoints and declared limits.
    /// Version 2 added the daemon source.
    pub fn fingerprint(&self) -> Result<StackFingerprint, String> {
        let encoded =
            serde_json::to_vec(self).map_err(|_| "cannot encode public stack configuration")?;
        Ok(fingerprint(&encoded, "v2"))
    }
}

fn fingerprint(encoded: &[u8], version: &str) -> StackFingerprint {
    use sha2::{Digest, Sha256};
    let mut hash = Sha256::new();
    hash.update(format!("mortsom-stack-{version}\0").as_bytes());
    hash.update((encoded.len() as u64).to_be_bytes());
    hash.update(encoded);
    StackFingerprint {
        algorithm: format!("sha256-mortsom-stack-{version}"),
        digest: hash
            .finalize()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect(),
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
            "daemon_source":{"kind":"git-ref","repository":"https://github.com/MostroP2P/mostro.git",
                "requested":"main","git_ref":"refs/heads/main","revision":"1111111111111111111111111111111111111111"},
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

    #[test]
    fn a_different_daemon_commit_changes_the_digest() {
        let snapshot: StackSnapshot = serde_json::from_value(serde_json::json!({
            "daemon_pubkey":"public", "relay_url":"ws://127.0.0.1:7000", "wallet_endpoints":{},
            "daemon_image":{"image_id":"sha256:abc","reference":"image","repository_digest":null},
            "daemon_source":{"kind":"git-ref","repository":"https://github.com/MostroP2P/mostro.git",
                "requested":"main","git_ref":"refs/heads/main","revision":"1111111111111111111111111111111111111111"},
            "declared_limits":{"order_expiration_secs":300,"hold_invoice_expiry_secs":120,
                "buyer_invoice_deadline_secs":120,"payout_invoice_min_expiry_secs":300}
        }))
        .unwrap();
        let mut moved = snapshot.clone();
        let super::super::DaemonProvenance::GitRef(selection) = &mut moved.daemon_source else {
            unreachable!()
        };
        selection.revision = "2".repeat(40);
        assert_ne!(
            snapshot.fingerprint().unwrap(),
            moved.fingerprint().unwrap()
        );
    }
}
