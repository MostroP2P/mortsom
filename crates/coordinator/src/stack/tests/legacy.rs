//! Compatibility never invents provenance or changes the recorded identity.
use super::*;

#[tokio::test]
async fn a_legacy_descriptor_is_read_without_rebuilding_or_backfilling_provenance() {
    let (_dir, layout) = scratch();
    let docker = ScriptedStack::fresh(&layout);
    let descriptor = up(&docker, &Publishes(true), &layout, &quick())
        .await
        .unwrap();
    let original = descriptor.snapshot().legacy_fingerprint().unwrap();
    let mut legacy = serde_json::to_value(&descriptor).unwrap();
    legacy["schema_version"] = 1.into();
    legacy["fingerprint"] = serde_json::to_value(&original).unwrap();
    legacy.as_object_mut().unwrap().remove("daemon_source");
    let bytes = serde_json::to_vec(&legacy).unwrap();
    std::fs::write(layout.descriptor_file(), &bytes).unwrap();
    let read = Descriptor::read(&layout).unwrap().unwrap();
    assert_eq!(read.daemon_source, DaemonProvenance::Unrecorded);
    assert!(read.snapshot().matches_recorded(&original).unwrap());
    assert_eq!(
        recorded_identity_for(layout.root(), &original),
        RecordedIdentity::Present(original.clone())
    );
    assert_eq!(bytes, std::fs::read(layout.descriptor_file()).unwrap());
    // Upgrading only the descriptor schema still denotes the original stack.
    descriptor.write(&layout).unwrap();
    assert_eq!(
        recorded_identity_for(layout.root(), &original),
        RecordedIdentity::Present(original)
    );
}

#[tokio::test]
async fn descriptor_reads_refuse_tampering_unknown_versions_and_missing_modern_provenance() {
    let (_dir, layout) = scratch();
    let docker = ScriptedStack::fresh(&layout);
    let descriptor = up(&docker, &Publishes(true), &layout, &quick())
        .await
        .unwrap();
    for alteration in ["fingerprint", "schema", "source", "limits"] {
        let mut value = serde_json::to_value(&descriptor).unwrap();
        match alteration {
            "fingerprint" => value["fingerprint"]["digest"] = "0".repeat(64).into(),
            "schema" => value["schema_version"] = 99.into(),
            "source" => {
                value.as_object_mut().unwrap().remove("daemon_source");
            }
            _ => value["declared_limits"]["order_expiration_secs"] = 123.into(),
        }
        std::fs::write(
            layout.descriptor_file(),
            serde_json::to_vec(&value).unwrap(),
        )
        .unwrap();
        assert!(Descriptor::read(&layout).is_err(), "{alteration}");
    }
}

#[test]
fn v1_fingerprint_matches_the_original_serialization_and_unknown_algorithms_refuse() {
    // Frozen v1 encoding: source provenance did not exist in this schema.
    let json = r#"{"daemon_pubkey":"public","relay_url":"ws://127.0.0.1:7000","wallet_endpoints":{},"daemon_image":{"image_id":"sha256:abc","reference":"image","repository_digest":null},"declared_limits":{"order_expiration_secs":300,"hold_invoice_expiry_secs":120,"buyer_invoice_deadline_secs":120,"fiat_window_secs":null,"payout_invoice_min_expiry_secs":300,"max_inflight_payouts_per_destination":null,"payout_dispatch_permits":null}}"#;
    let snapshot: StackSnapshot = serde_json::from_str(json).unwrap();
    use sha2::{Digest, Sha256};
    let mut hash = Sha256::new();
    hash.update(b"mortsom-stack-v1\0");
    hash.update((json.len() as u64).to_be_bytes());
    hash.update(json.as_bytes());
    let original = StackFingerprint {
        algorithm: "sha256-mortsom-stack-v1".into(),
        digest: hash
            .finalize()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect(),
    };
    assert!(snapshot.matches_recorded(&original).unwrap());
    let mut changed = snapshot.clone();
    changed.daemon_pubkey = "different".into();
    assert!(!changed.matches_recorded(&original).unwrap());
    assert!(
        snapshot
            .matches_recorded(&StackFingerprint {
                algorithm: "unknown".into(),
                ..original
            })
            .is_err()
    );
}
