#![allow(clippy::unwrap_used)]
use super::*;
use std::os::unix::fs::PermissionsExt;
fn manifest() -> RecoveryManifest {
    RecoveryManifest {
        run_id: RunId::new(),
        stack: StackFingerprint {
            algorithm: "test".into(),
            digest: "a".repeat(64),
        },
        platform: Platform::Linux,
        bundle: BundleFingerprint {
            algorithm: "test".into(),
            digest: "b".repeat(64),
            files: 1,
            bytes: 3,
        },
        build: BuildProvenance::prebuilt(),
        executable: Some("mostro".into()),
        identities: BTreeMap::new(),
        origin: None,
        events: vec![],
        closed: false,
        resolution_verified: false,
    }
}
#[test]
fn replay_survives_reopening_and_blocks_new_runs_until_explicit_resolution() {
    let base = tempfile::tempdir().unwrap();
    let journal = FileJournal::create(base.path(), manifest()).unwrap();
    journal
        .record(RecoveryEvent::SubmissionStarted {
            actor: mortsom_common::Actor::Alice,
            known_orders: vec![],
        })
        .unwrap();
    let root = journal.root().to_path_buf();
    drop(journal);
    let reopened = FileJournal::open(&root).unwrap();
    assert_eq!(reopened.snapshot().events.len(), 1);
    assert!(FileJournal::refuse_pending(base.path()).is_err());
    reopened.close().unwrap();
    FileJournal::refuse_pending(base.path()).unwrap();
}
#[test]
fn private_manifest_does_not_follow_links_or_accept_world_readable_files() {
    let base = tempfile::tempdir().unwrap();
    let journal = FileJournal::create(base.path(), manifest()).unwrap();
    let path = journal.root().join("manifest.json");
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
    assert!(FileJournal::open(journal.root()).is_err());
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
    let original = journal.root().join("original");
    std::fs::rename(&path, &original).unwrap();
    std::os::unix::fs::symlink(original, &path).unwrap();
    assert!(FileJournal::open(journal.root()).is_err());
}
#[test]
fn failed_durable_write_does_not_change_the_in_memory_history() {
    let base = tempfile::tempdir().unwrap();
    let journal = FileJournal::create(base.path(), manifest()).unwrap();
    std::fs::remove_file(journal.root().join("manifest.json")).unwrap();
    std::fs::create_dir(journal.root().join("manifest.json")).unwrap();
    assert!(
        journal
            .record(RecoveryEvent::OrderResolved {
                order_id: "o1".into()
            })
            .is_err()
    );
    assert!(journal.snapshot().events.is_empty());
}
#[test]
fn only_original_alice_and_bob_identities_can_authorize_recovery() {
    let base = tempfile::tempdir().unwrap();
    let journal = FileJournal::create(base.path(), manifest()).unwrap();
    assert!(
        journal
            .set_actors(
                BTreeMap::from([
                    ("carol".into(), "key1".into()),
                    ("dave".into(), "key2".into())
                ]),
                None
            )
            .is_err()
    );
}

#[test]
fn repeated_payment_intents_cannot_authorize_a_second_wallet_submission() {
    let base = tempfile::tempdir().unwrap();
    let journal = FileJournal::create(base.path(), manifest()).unwrap();
    let event = RecoveryEvent::PaymentExpected {
        order_id: "o1".into(),
        wallet: "alice".into(),
        hash: "a".repeat(64),
        incoming: false,
        amount_sats: 1000,
    };
    journal.record(event.clone()).unwrap();
    assert!(journal.record(event).is_err());
    assert!(
        journal
            .record(RecoveryEvent::PaymentExpected {
                order_id: "o1".into(),
                wallet: "alice".into(),
                hash: "b".repeat(64),
                incoming: false,
                amount_sats: 1000
            })
            .is_err()
    );
    assert_eq!(
        FileJournal::open(journal.root())
            .unwrap()
            .snapshot()
            .events
            .len(),
        1
    );
}

/// A payout invoice withdrawn on purpose (refused by the daemon, or
/// cancelled so the daemon's payment fails) is not a replay when the
/// buyer prepares a replacement for the same order; the withdrawal is
/// what makes the second intent legitimate, and it must name a prepared
/// payment.
#[test]
fn a_superseded_payment_lets_a_replacement_be_prepared_for_the_same_order() {
    let base = tempfile::tempdir().unwrap();
    let journal = FileJournal::create(base.path(), manifest()).unwrap();
    let expected = |hash: char| RecoveryEvent::PaymentExpected {
        order_id: "o1".into(),
        wallet: "bob".into(),
        hash: hash.to_string().repeat(64),
        incoming: true,
        amount_sats: 1000,
    };
    assert!(
        journal
            .record(RecoveryEvent::PaymentSuperseded {
                order_id: "o1".into(),
                wallet: "bob".into(),
                hash: "a".repeat(64),
            })
            .is_err(),
        "nothing prepared yet"
    );
    journal.record(expected('a')).unwrap();
    assert!(journal.record(expected('b')).is_err(), "still a replay");
    journal
        .record(RecoveryEvent::PaymentSuperseded {
            order_id: "o1".into(),
            wallet: "bob".into(),
            hash: "a".repeat(64),
        })
        .unwrap();
    journal.record(expected('b')).unwrap();
    assert!(
        journal.record(expected('a')).is_err(),
        "the old hash stays known"
    );
    assert!(
        journal.record(expected('c')).is_err(),
        "the replacement is live"
    );
    assert!(superseded(
        &journal.snapshot().events,
        "bob",
        &"a".repeat(64)
    ));
    assert!(!superseded(
        &journal.snapshot().events,
        "alice",
        &"a".repeat(64)
    ));
    assert!(!superseded(
        &journal.snapshot().events,
        "bob",
        &"b".repeat(64)
    ));
}

#[test]
fn verified_resolution_survives_a_crash_between_profile_and_bundle_deletion() {
    let base = tempfile::tempdir().unwrap();
    let journal = FileJournal::create(base.path(), manifest()).unwrap();
    std::fs::create_dir(journal.root().join("bundle")).unwrap();
    journal.confirm_resolution().unwrap();
    let reopened = FileJournal::open(journal.root()).unwrap();
    assert!(reopened.snapshot().resolution_verified);
    assert!(!reopened.snapshot().closed);
    crate::run::recovery::remove_storage(&reopened).unwrap();
    assert!(FileJournal::open(journal.root()).unwrap().snapshot().closed);
    assert!(!journal.root().join("bundle").exists());
}

#[test]
fn a_fifo_manifest_is_rejected_without_waiting_for_a_writer() {
    use rustix::fs::{CWD, FileType, Mode, mknodat};
    let base = tempfile::tempdir().unwrap();
    let journal = FileJournal::create(base.path(), manifest()).unwrap();
    let path = journal.root().join("manifest.json");
    std::fs::remove_file(&path).unwrap();
    mknodat(CWD, &path, FileType::Fifo, Mode::RUSR | Mode::WUSR, 0).unwrap();
    assert!(FileJournal::open(journal.root()).is_err());
}
#[test]
fn pending_recovery_message_names_the_exact_cleanup_command() {
    // Arrange
    let base = tempfile::tempdir().unwrap();
    let journal = FileJournal::create(base.path(), manifest()).unwrap();
    let run_id = journal.snapshot().run_id.as_str().to_string();
    drop(journal);

    // Act
    let pending = FileJournal::refuse_pending(base.path()).unwrap_err();

    // Assert
    let plain = pending.message(false);
    assert_eq!(
        plain,
        format!(
            "run {run_id} retains unresolved recovery state; run `mortsom cleanup {run_id}` before starting another desktop scenario"
        )
    );
    assert!(!plain.contains('\u{1b}'), "{plain:?}");
    let styled = pending.message(true);
    assert!(
        styled.contains(&format!("\u{1b}[1mmortsom cleanup {run_id}\u{1b}[0m")),
        "{styled:?}"
    );
    assert_eq!(String::from(pending), plain);
}
