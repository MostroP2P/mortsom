#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use async_trait::async_trait;
use mortsom_common::{Lane, RunId};

use super::*;
use crate::recovery_store::{
    RecoveryEnvironment, RecoveryManifest, RecoveryOutcome, WalletBackend,
};
use crate::stack::{ProjectOwner, RecordedIdentity};

fn fingerprint(digit: char) -> StackFingerprint {
    StackFingerprint {
        algorithm: "sha256-mortsom-stack-v1".into(),
        digest: digit.to_string().repeat(64),
    }
}

fn environment(stack_dir: &Path, artifacts_dir: &Path) -> RecoveryEnvironment {
    RecoveryEnvironment {
        lane: Lane::Local,
        allow_mainnet: false,
        artifacts_dir: Some(artifacts_dir.to_path_buf()),
        stack_dir: Some(stack_dir.to_path_buf()),
        wallet_backend: WalletBackend::StackLnd,
        node_networks: BTreeMap::from([
            ("alice".into(), "regtest".into()),
            ("bob".into(), "regtest".into()),
        ]),
    }
}

fn manifest(run_id: RunId, environment: Option<RecoveryEnvironment>) -> RecoveryManifest {
    RecoveryManifest {
        run_id,
        stack: fingerprint('a'),
        platform: mortsom_desktop_driver::Platform::Linux,
        bundle: crate::fingerprint::BundleFingerprint {
            algorithm: "test".into(),
            digest: "b".repeat(64),
            files: 1,
            bytes: 3,
        },
        build: crate::provenance::BuildProvenance::prebuilt(),
        executable: Some("mostro".into()),
        identities: BTreeMap::new(),
        origin: None,
        events: vec![],
        closed: false,
        resolution_verified: false,
        outcome: Some(RecoveryOutcome::Pending),
        environment,
    }
}

struct Fixture {
    dir: tempfile::TempDir,
    run_id: RunId,
    journal: PathBuf,
}

impl Fixture {
    fn new(tweak: impl FnOnce(&mut RecoveryEnvironment)) -> Self {
        Self::with(|stack, artifacts| {
            let mut environment = environment(stack, artifacts);
            tweak(&mut environment);
            Some(environment)
        })
    }

    /// `environment` receives the stack directory and the artifacts
    /// directory the run is recorded to have written to.
    fn with(environment: impl FnOnce(&Path, &Path) -> Option<RecoveryEnvironment>) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let stack = dir.path().join("stack");
        let run_id = RunId::new();
        let base = dir.path().canonicalize().unwrap().join("recovery");
        let artifacts = dir.path().join("artifacts");
        std::fs::create_dir_all(artifacts.join(run_id.as_str())).unwrap();
        let journal = FileJournal::create(
            &base,
            manifest(run_id.clone(), environment(&stack, &artifacts)),
        )
        .unwrap();
        for part in ["actors/alice", "bundle"] {
            std::fs::create_dir_all(journal.root().join(part)).unwrap();
        }
        let journal = journal.root().to_path_buf();
        Self {
            dir,
            run_id,
            journal,
        }
    }

    fn request(&self) -> AbandonRequest {
        AbandonRequest {
            run_id: self.run_id.clone(),
            reason: AbandonReason::StackDestroyed,
            confirm_run: self.run_id.as_str().to_owned(),
        }
    }

    fn artifacts(&self) -> PathBuf {
        self.dir.path().join("artifacts").join(self.run_id.as_str())
    }

    fn selected(&self) -> PathBuf {
        self.dir.path().join("selected")
    }

    async fn abandon_with(
        &self,
        request: &AbandonRequest,
        inspector: &dyn StackInspector,
        storage: &dyn RetainedStorage,
    ) -> Result<AbandonReport, String> {
        let selected = self.selected();
        let paths = AbandonPaths {
            journal: &self.journal,
            selected_stack: &selected,
        };
        abandon(request, &paths, inspector, storage, chrono::Utc::now()).await
    }

    fn state(&self) -> RecoveryOutcome {
        FileJournal::open(&self.journal).unwrap().snapshot().state()
    }

    fn storage_intact(&self) -> bool {
        self.journal.join("actors").exists() && self.journal.join("bundle").exists()
    }
}

/// Facts where the original stack is gone and a new one replaced it.
fn replaced() -> LiveFacts {
    LiveFacts {
        recorded: ("/stack".into(), RecordedIdentity::Present(fingerprint('c'))),
        selected: ("/stack".into(), RecordedIdentity::Present(fingerprint('c'))),
        project: ProjectOwner::Directory("/stack".into()),
        project_identity: Some(RecordedIdentity::Present(fingerprint('c'))),
    }
}

fn nothing_left() -> LiveFacts {
    LiveFacts {
        recorded: ("/stack".into(), RecordedIdentity::Absent),
        selected: ("/other".into(), RecordedIdentity::Absent),
        project: ProjectOwner::Absent,
        project_identity: None,
    }
}

struct FakeInspector(LiveFacts);

#[async_trait]
impl StackInspector for FakeInspector {
    async fn facts(&self, _: &Path, _: &Path) -> Result<LiveFacts, String> {
        Ok(self.0.clone())
    }
}

/// Removes like the real storage, but first records what was durable.
#[derive(Default)]
struct FakeStorage {
    fail: bool,
    observed: Mutex<Vec<(bool, bool)>>,
    artifact: Mutex<Option<PathBuf>>,
}

impl RetainedStorage for FakeStorage {
    fn remove(&self, root: &Path) -> Removal {
        let journal = FileJournal::open(root).unwrap().snapshot();
        let evidence = root.join(JOURNAL_FILE).is_file()
            && self
                .artifact
                .lock()
                .unwrap()
                .as_ref()
                .is_none_or(|path| path.is_file());
        self.observed
            .lock()
            .unwrap()
            .push((evidence, !journal.state().is_pending()));
        if self.fail {
            return Removal {
                removed: Vec::new(),
                error: Some("injected removal failure".into()),
            };
        }
        let mut removed = Vec::new();
        for name in ["actors", "bundle"] {
            let path = root.join(name);
            if path.exists() {
                std::fs::remove_dir_all(&path).unwrap();
                removed.push(path);
            }
        }
        Removal {
            removed,
            error: None,
        }
    }
}

#[tokio::test]
async fn a_destroyed_stack_owned_regtest_run_is_abandoned_with_durable_evidence() {
    let fixture = Fixture::new(|_| {});
    let storage = FakeStorage::default();
    *storage.artifact.lock().unwrap() = Some(fixture.artifacts().join(ARTIFACT_FILE));

    let report = fixture
        .abandon_with(&fixture.request(), &FakeInspector(replaced()), &storage)
        .await
        .unwrap();

    assert!(matches!(
        fixture.state(),
        RecoveryOutcome::AbandonedRegtest {
            reason: AbandonReason::StackDestroyed,
            ..
        }
    ));
    assert_eq!(
        *storage.observed.lock().unwrap(),
        vec![(true, true)],
        "evidence and the terminal record precede removal"
    );
    assert!(!fixture.storage_intact());
    assert!(fixture.journal.join("manifest.json").is_file());
    let evidence = read_artifact(&fixture.artifacts()).unwrap();
    assert_eq!(evidence, report.evidence);
    assert!(evidence.current_stack_present);
    assert!(!evidence.resolution_verified);
    assert_eq!(evidence.original_stack, fingerprint('a'));
    assert!(evidence.statement.contains("NOT verified"));
    assert!(!evidence.checks_passed.is_empty());
    let text = report.render();
    assert!(text.contains("preserved:") && text.contains("removed:"));
    assert!(text.contains("FAILED run"), "{text}");
    let base = fixture.journal.parent().unwrap();
    FileJournal::refuse_pending(base).expect("an abandoned journal no longer blocks new runs");
}

#[tokio::test]
async fn a_stack_with_nothing_left_is_destroyed_without_a_current_stack() {
    let fixture = Fixture::new(|_| {});
    let report = fixture
        .abandon_with(
            &fixture.request(),
            &FakeInspector(nothing_left()),
            &FakeStorage::default(),
        )
        .await
        .unwrap();
    assert!(!report.evidence.current_stack_present);
}

async fn assert_refused(facts: LiveFacts, expected: &str) {
    let fixture = Fixture::new(|_| {});
    let error = fixture
        .abandon_with(
            &fixture.request(),
            &FakeInspector(facts),
            &FakeStorage::default(),
        )
        .await
        .unwrap_err();
    assert!(error.contains(expected), "{error}");
    assert!(fixture.state().is_pending());
    assert!(fixture.storage_intact());
    assert!(read_artifact(&fixture.artifacts()).is_none());
}

#[tokio::test]
async fn a_live_original_stack_is_refused_and_directed_to_cleanup() {
    let live_somewhere = [
        LiveFacts {
            recorded: ("/stack".into(), RecordedIdentity::Present(fingerprint('a'))),
            ..replaced()
        },
        LiveFacts {
            selected: ("/sel".into(), RecordedIdentity::Present(fingerprint('a'))),
            ..nothing_left()
        },
        LiveFacts {
            project: ProjectOwner::Directory("/foreign".into()),
            project_identity: Some(RecordedIdentity::Present(fingerprint('a'))),
            ..nothing_left()
        },
    ];
    for facts in live_somewhere {
        assert_refused(facts, "mortsom cleanup").await;
    }
}

#[tokio::test]
async fn an_unprovable_original_stack_is_refused() {
    let unprovable = [
        LiveFacts {
            project: ProjectOwner::Unprovable("volumes remain".into()),
            ..nothing_left()
        },
        LiveFacts {
            recorded: ("/stack".into(), RecordedIdentity::Unreadable("bad".into())),
            ..nothing_left()
        },
        LiveFacts {
            project: ProjectOwner::Directory("/half-built".into()),
            project_identity: Some(RecordedIdentity::Absent),
            ..nothing_left()
        },
    ];
    for facts in unprovable {
        assert_refused(facts, "cannot prove").await;
    }
}

#[tokio::test]
async fn non_regtest_external_mainnet_canary_and_legacy_runs_are_refused() {
    let fixtures = [
        Fixture::new(|e| e.lane = Lane::Canary),
        Fixture::new(|e| e.allow_mainnet = true),
        Fixture::new(|e| e.wallet_backend = WalletBackend::External),
        Fixture::new(|e| {
            e.node_networks.insert("bob".into(), "mainnet".into());
        }),
        Fixture::new(|e| e.node_networks.clear()),
        Fixture::new(|e| e.stack_dir = None),
        Fixture::with(|_, _| None),
    ];
    for fixture in fixtures {
        let error = fixture
            .abandon_with(
                &fixture.request(),
                &FakeInspector(nothing_left()),
                &FakeStorage::default(),
            )
            .await
            .unwrap_err();
        assert!(error.contains("cannot be"), "{error}");
        assert!(fixture.state().is_pending() && fixture.storage_intact());
    }
}

#[tokio::test]
async fn a_wrong_confirmation_is_refused_before_anything_is_read() {
    let fixture = Fixture::new(|_| {});
    for confirm in ["", "other-run", &format!("{} ", fixture.run_id)] {
        let request = AbandonRequest {
            confirm_run: confirm.to_owned(),
            ..fixture.request()
        };
        let error = fixture
            .abandon_with(
                &request,
                &FakeInspector(nothing_left()),
                &FakeStorage::default(),
            )
            .await
            .unwrap_err();
        assert!(error.contains("--confirm-run"), "{error}");
    }
    assert!(fixture.state().is_pending() && fixture.storage_intact());
}

#[tokio::test]
async fn an_evidence_write_failure_leaves_the_journal_pending_and_storage_intact() {
    let fixture = Fixture::new(|_| {});
    // A directory where the protected record must go makes its write fail.
    std::fs::create_dir(fixture.journal.join(JOURNAL_FILE)).unwrap();
    let storage = FakeStorage::default();

    let error = fixture
        .abandon_with(&fixture.request(), &FakeInspector(replaced()), &storage)
        .await
        .unwrap_err();

    assert!(error.contains("cannot persist"), "{error}");
    assert!(fixture.state().is_pending());
    assert!(fixture.storage_intact());
    assert!(storage.observed.lock().unwrap().is_empty());
    let base = fixture.journal.parent().unwrap();
    assert!(FileJournal::refuse_pending(base).is_err());
    assert!(
        matches!(abandonment(&fixture.artifacts()), Abandonment::Prepared(_)),
        "the prepared public record already fails reports closed"
    );
}

#[tokio::test]
async fn a_removal_failure_is_reported_and_a_rerun_retries_it_idempotently() {
    let fixture = Fixture::new(|_| {});
    let failing = FakeStorage {
        fail: true,
        ..FakeStorage::default()
    };
    let first = fixture
        .abandon_with(&fixture.request(), &FakeInspector(replaced()), &failing)
        .await
        .unwrap();
    assert!(first.removal_error.is_some());
    assert!(first.render().contains("NOT removed"));
    assert!(!fixture.state().is_pending());
    assert!(fixture.storage_intact());

    // The live stack no longer matters: the journal is already terminal.
    let live = LiveFacts {
        recorded: ("/stack".into(), RecordedIdentity::Present(fingerprint('a'))),
        ..replaced()
    };
    let second = fixture
        .abandon_with(
            &fixture.request(),
            &FakeInspector(live),
            &FakeStorage::default(),
        )
        .await
        .unwrap();
    assert!(second.already_abandoned);
    assert_eq!(second.evidence, first.evidence);
    assert!(second.removal_error.is_none());
    assert!(!fixture.storage_intact());
    assert!(second.render().contains("was already abandoned"));

    let third = fixture
        .abandon_with(
            &fixture.request(),
            &FakeInspector(replaced()),
            &FakeStorage::default(),
        )
        .await
        .unwrap();
    assert!(third.removed.is_empty());
    assert!(third.render().contains("nothing"));
}

#[tokio::test]
async fn a_resolved_journal_cannot_be_abandoned() {
    let fixture = Fixture::new(|_| {});
    FileJournal::open(&fixture.journal)
        .unwrap()
        .close()
        .unwrap();
    let error = fixture
        .abandon_with(
            &fixture.request(),
            &FakeInspector(nothing_left()),
            &FakeStorage::default(),
        )
        .await
        .unwrap_err();
    assert!(error.contains("already resolved"), "{error}");
    assert_eq!(fixture.state(), RecoveryOutcome::Resolved);
}

#[test]
fn real_storage_removes_the_bundle_and_refuses_an_unverified_profile() {
    let fixture = Fixture::new(|_| {});
    std::fs::remove_dir_all(fixture.journal.join("actors")).unwrap();
    let removal = RealStorage.remove(&fixture.journal);
    assert_eq!(removal.removed, vec![fixture.journal.join("bundle")]);
    assert_eq!(removal.error, None);

    let fixture = Fixture::new(|_| {});
    let refused = RealStorage.remove(&fixture.journal);
    assert!(refused.error.is_some());
    assert!(refused.removed.is_empty());
    assert!(fixture.storage_intact());
}

fn pending(fixture: &Fixture) -> Vec<RecoveryManifest> {
    vec![FileJournal::open(&fixture.journal).unwrap().snapshot()]
}

#[tokio::test]
async fn stack_lifecycle_refuses_every_operation_while_the_current_stack_owns_recovery() {
    let fixture = Fixture::new(|_| {});
    let live = LiveFacts {
        selected: ("/stack".into(), RecordedIdentity::Present(fingerprint('a'))),
        ..nothing_left()
    };
    for operation in [
        LifecycleOperation::Down,
        LifecycleOperation::Up { replace: false },
        LifecycleOperation::Up { replace: true },
    ] {
        let error = lifecycle_guard(
            operation,
            &pending(&fixture),
            &fixture.selected(),
            &FakeInspector(live.clone()),
        )
        .await
        .unwrap_err();
        assert!(
            error.contains(&format!("mortsom cleanup {}", fixture.run_id)),
            "{error}"
        );
    }
    let replace = lifecycle_guard(
        LifecycleOperation::Up { replace: true },
        &pending(&fixture),
        &fixture.selected(),
        &FakeInspector(LiveFacts {
            project: ProjectOwner::Directory("/foreign".into()),
            project_identity: Some(RecordedIdentity::Present(fingerprint('a'))),
            ..nothing_left()
        }),
    )
    .await
    .unwrap_err();
    assert!(replace.contains("--replace"), "{replace}");
}

#[tokio::test]
async fn stack_lifecycle_names_the_abandon_command_only_when_its_gates_pass() {
    let fixture = Fixture::new(|_| {});
    let notes = lifecycle_guard(
        LifecycleOperation::Up { replace: false },
        &pending(&fixture),
        &fixture.selected(),
        &FakeInspector(nothing_left()),
    )
    .await
    .unwrap();
    assert_eq!(notes.len(), 1);
    assert!(
        notes[0].contains(&command_for(&fixture.run_id)),
        "{}",
        notes[0]
    );

    let canary = Fixture::new(|e| e.lane = Lane::Canary);
    let notes = lifecycle_guard(
        LifecycleOperation::Down,
        &pending(&canary),
        &canary.selected(),
        &FakeInspector(nothing_left()),
    )
    .await
    .unwrap();
    assert!(!notes[0].contains("recovery abandon"), "{}", notes[0]);
}

#[tokio::test]
async fn stack_lifecycle_refuses_while_a_pending_journal_cannot_be_proved_gone() {
    let fixture = Fixture::new(|_| {});
    let unprovable = LiveFacts {
        project: ProjectOwner::Unprovable("volumes".into()),
        ..nothing_left()
    };
    let error = lifecycle_guard(
        LifecycleOperation::Down,
        &pending(&fixture),
        &fixture.selected(),
        &FakeInspector(unprovable),
    )
    .await
    .unwrap_err();
    assert!(error.contains("cannot be proved"), "{error}");
    assert!(
        error.contains(&format!("mortsom cleanup {}", fixture.run_id)),
        "{error}"
    );
}

#[tokio::test]
async fn evidence_lands_only_in_the_recorded_artifact_directory() {
    let fixture = Fixture::new(|_| {});
    let report = fixture
        .abandon_with(
            &fixture.request(),
            &FakeInspector(nothing_left()),
            &FakeStorage::default(),
        )
        .await
        .unwrap();
    assert!(matches!(
        abandonment(&fixture.artifacts()),
        Abandonment::Finalized(_)
    ));
    assert!(report.evidence.finalized);
    assert!(
        report
            .preserved
            .contains(&fixture.artifacts().join(ARTIFACT_FILE))
    );

    // A journal that never recorded where its run wrote is refused: the
    // selected configuration proves nothing about the original tree.
    let legacy = Fixture::new(|e| e.artifacts_dir = None);
    let error = legacy
        .abandon_with(
            &legacy.request(),
            &FakeInspector(nothing_left()),
            &FakeStorage::default(),
        )
        .await
        .unwrap_err();
    assert!(
        error.contains("predates the recorded artifact directory"),
        "{error}"
    );
    assert!(legacy.state().is_pending());
    assert!(!legacy.journal.join(JOURNAL_FILE).exists());
    assert!(matches!(
        abandonment(&legacy.artifacts()),
        Abandonment::None
    ));
}

#[tokio::test]
async fn a_missing_run_artifact_directory_refuses_before_the_journal_changes() {
    let fixture = Fixture::new(|_| {});
    std::fs::remove_dir_all(fixture.artifacts()).unwrap();

    let error = fixture
        .abandon_with(
            &fixture.request(),
            &FakeInspector(nothing_left()),
            &FakeStorage::default(),
        )
        .await
        .unwrap_err();

    assert!(error.contains("artifact directory"), "{error}");
    assert!(fixture.state().is_pending());
    assert!(!fixture.journal.join(JOURNAL_FILE).exists());
    assert!(fixture.storage_intact());
}

/// A coordinator killed after the journal turned terminal but before the
/// public record was finalized: reports already fail closed on the prepared
/// record, and a rerun finalizes it without re-proving anything.
#[tokio::test]
async fn a_terminal_journal_with_a_prepared_record_is_finalized_on_rerun() {
    let fixture = Fixture::new(|_| {});
    write_incomplete_test_artifacts(&fixture.artifacts(), false);
    let journal = FileJournal::open(&fixture.journal).unwrap();
    let prepared = read_artifact(&fixture.artifacts()).unwrap();
    std::fs::write(
        fixture.journal.join(JOURNAL_FILE),
        serde_json::to_string(&prepared).unwrap(),
    )
    .unwrap();
    journal
        .abandon(AbandonReason::StackDestroyed, chrono::Utc::now())
        .unwrap();
    assert!(abandonment(&fixture.artifacts()).is_abandoned());

    let live = LiveFacts {
        recorded: ("/stack".into(), RecordedIdentity::Present(fingerprint('a'))),
        ..replaced()
    };
    let report = fixture
        .abandon_with(
            &fixture.request(),
            &FakeInspector(live),
            &FakeStorage::default(),
        )
        .await
        .unwrap();

    assert!(report.already_abandoned);
    assert!(report.evidence.finalized);
    assert!(matches!(
        abandonment(&fixture.artifacts()),
        Abandonment::Finalized(_)
    ));
    assert!(!fixture.storage_intact());
}

/// A coordinator killed after the prepared record but before the journal
/// turned terminal: the run fails closed meanwhile, and a rerun proves the
/// gates again before finishing.
#[tokio::test]
async fn a_pending_journal_with_a_prepared_record_is_reproved_on_rerun() {
    let fixture = Fixture::new(|_| {});
    write_incomplete_test_artifacts(&fixture.artifacts(), false);
    assert!(fixture.state().is_pending());

    let live = LiveFacts {
        recorded: ("/stack".into(), RecordedIdentity::Present(fingerprint('a'))),
        ..replaced()
    };
    let refused = fixture
        .abandon_with(
            &fixture.request(),
            &FakeInspector(live),
            &FakeStorage::default(),
        )
        .await
        .unwrap_err();
    assert!(refused.contains("still exists"), "{refused}");
    assert!(matches!(
        abandonment(&fixture.artifacts()),
        Abandonment::Prepared(_)
    ));

    let report = fixture
        .abandon_with(
            &fixture.request(),
            &FakeInspector(nothing_left()),
            &FakeStorage::default(),
        )
        .await
        .unwrap();
    assert!(!report.already_abandoned);
    assert!(matches!(
        abandonment(&fixture.artifacts()),
        Abandonment::Finalized(_)
    ));
}

#[test]
fn every_present_record_counts_as_abandonment_and_only_a_missing_one_does_not() {
    let dir = tempfile::tempdir().unwrap();
    let run = dir.path().join("run");
    assert_eq!(abandonment(&run), Abandonment::None);
    std::fs::create_dir_all(&run).unwrap();
    assert_eq!(abandonment(&run), Abandonment::None);

    write_incomplete_test_artifacts(&run, false);
    assert!(matches!(abandonment(&run), Abandonment::Prepared(_)));
    write_test_artifact(&run);
    assert!(matches!(abandonment(&run), Abandonment::Finalized(_)));
    write_incomplete_test_artifacts(&run, true);
    let unreadable = abandonment(&run);
    assert!(
        matches!(unreadable, Abandonment::Unreadable(_)),
        "{unreadable:?}"
    );
    assert!(unreadable.is_abandoned());
    assert!(unreadable.evidence().is_none());
}

/// Cleanup that verifies resolution after an abandonment died before its
/// journal turned terminal discards the prepared record; anything else next
/// to a resolved journal stays, so the contradiction is investigated.
#[test]
fn a_resolved_journal_discards_only_a_prepared_record() {
    let fixture = Fixture::new(|_| {});
    let journal = FileJournal::open(&fixture.journal).unwrap();
    write_incomplete_test_artifacts(&fixture.artifacts(), false);
    assert!(discard_prepared(&journal.snapshot()).is_ok());
    assert!(
        abandonment(&fixture.artifacts()).is_abandoned(),
        "a pending journal discards nothing"
    );

    journal.close().unwrap();
    discard_prepared(&journal.snapshot()).unwrap();
    assert_eq!(abandonment(&fixture.artifacts()), Abandonment::None);

    for corrupt in [false, true] {
        if corrupt {
            write_incomplete_test_artifacts(&fixture.artifacts(), true);
        } else {
            write_test_artifact(&fixture.artifacts());
        }
        discard_prepared(&journal.snapshot()).unwrap();
        assert!(
            abandonment(&fixture.artifacts()).is_abandoned(),
            "{corrupt}"
        );
    }
}

#[test]
fn the_hint_requires_a_destroyed_stack() {
    let fixture = Fixture::new(|_| {});
    let manifest = pending(&fixture).remove(0);
    assert!(abandon_hint(&manifest, &StackPresence::Live("x".into())).is_none());
    assert!(abandon_hint(&manifest, &StackPresence::Unprovable("x".into())).is_none());
    let hint = abandon_hint(
        &manifest,
        &StackPresence::Destroyed {
            current_stack_present: true,
        },
    )
    .unwrap();
    assert!(hint.contains("destroyed Mortsom regtest stack"));
    assert!(hint.contains("--reason stack-destroyed --confirm-run"));
}
