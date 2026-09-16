//! Real runner boundary with fake stack inspection, wallets and public UI.
use super::*;
use crate::recovery_abandon::{LiveFacts, StackInspector};
use crate::recovery_isolation::{IsolationContext, ValidatedIsolation, authorize};
use crate::recovery_store::FileJournal;
use crate::stack::{ProjectOwner, RecordedIdentity};
use mortsom_desktop_driver::Platform;

struct Inspector(LiveFacts);
#[async_trait]
impl StackInspector for Inspector {
    async fn facts(&self, _: &Path, _: &Path) -> Result<LiveFacts, String> {
        Ok(self.0.clone())
    }
}

fn inspector(world: &World) -> Inspector {
    inspector_for(world, &stack())
}

fn inspector_for(world: &World, stack: &crate::stack::StackSnapshot) -> Inspector {
    let dir = world.dir.path().to_path_buf();
    let identity = RecordedIdentity::Present(stack.fingerprint().unwrap());
    Inspector(LiveFacts {
        recorded: (dir.clone(), identity.clone()),
        selected: (dir.clone(), identity.clone()),
        project: ProjectOwner::Directory(dir),
        project_identity: Some(identity),
    })
}

fn legacy(world: &World) -> FileJournal {
    let saved = recovery::saved_run(world);
    let base = FileJournal::directory(&world.dir.path().join("home")).unwrap();
    let mut manifest = saved.snapshot();
    manifest.stack = crate::stack::StackFingerprint {
        algorithm: "sha256-mortsom-stack-v1".into(),
        digest: "a".repeat(64),
    };
    let journal = FileJournal::create(&base, manifest).unwrap();
    for actor in Actor::ALL {
        crate::artifacts::persist::create_directory(
            &journal.root().join("actors").join(actor.as_str()),
        )
        .unwrap();
    }
    journal
}

async fn authorization(
    world: &World,
    ids: &[RunId],
    inspector: &Inspector,
    network: &'static str,
) -> Result<ValidatedIsolation, String> {
    let base = FileJournal::directory(&world.dir.path().join("home")).unwrap();
    let stack = stack();
    let wallets = nodes_on(Arc::new(Ledger::default()), network);
    let context = IsolationContext {
        base: &base,
        confirmed: ids,
        config: &world.config,
        stack: &stack,
        stack_dir: world.dir.path(),
    };
    authorize(&context, inspector, &wallets)
        .await
        .map_err(|error| error.to_string())
}

#[tokio::test]
async fn legacy_continuation_runs_with_fresh_actors_and_keeps_old_recovery_pending() {
    mortsom_scenarios::link();
    for (platform, unrecorded) in [
        (Platform::Linux, false),
        (Platform::Web, false),
        (Platform::Linux, true),
        (Platform::Web, true),
    ] {
        let world = world(false);
        let journal = legacy(&world);
        let before = std::fs::read(journal.root().join("manifest.json")).unwrap();
        let id = journal.snapshot().run_id;
        let mut options = opts_in(&world, &["happy_sell"]);
        options.desktop.platform = platform;
        if platform == Platform::Web {
            let web = world.dir.path().join("web");
            std::fs::create_dir_all(web.join("pkg")).unwrap();
            for name in [
                "index.html",
                "flutter_bootstrap.js",
                "main.dart.js",
                "pkg/rust.js",
                "pkg/rust_bg.wasm",
            ] {
                std::fs::write(web.join(name), "fixture").unwrap();
            }
            options.desktop.bundle =
                Arc::new(crate::fingerprint::CapturedBundle::capture(&web).unwrap());
            options.desktop.artifact = options.desktop.bundle.root().to_path_buf();
        }
        options.stack_dir = Some(world.dir.path().to_path_buf());
        let mut current = stack();
        if unrecorded {
            current.daemon_source = crate::stack::DaemonProvenance::Unrecorded;
        }
        let base = FileJournal::directory(&world.dir.path().join("home")).unwrap();
        let context = IsolationContext {
            base: &base,
            confirmed: std::slice::from_ref(&id),
            config: &world.config,
            stack: &current,
            stack_dir: world.dir.path(),
        };
        options.recovery_isolation = Some(
            authorize(
                &context,
                &inspector_for(&world, &current),
                &nodes_on(Arc::new(Ledger::default()), "regtest"),
            )
            .await
            .unwrap(),
        );
        options.stack = Some(current);
        let summary = run(&world, Arc::new(FakeAdapter::full()), options, false)
            .await
            .unwrap();
        assert!(summary.passed, "{:?}", summary.checks);
        if unrecorded {
            let environment: serde_json::Value = serde_json::from_slice(
                &std::fs::read(
                    world
                        .dir
                        .path()
                        .join("artifacts")
                        .join(summary.run_id.as_str())
                        .join("environment.json"),
                )
                .unwrap(),
            )
            .unwrap();
            assert_eq!(environment["stack"]["daemon_source"]["kind"], "unrecorded");
        }
        assert_eq!(summary.results[0].attempts, 1);
        let evidence = world
            .dir
            .path()
            .join("artifacts")
            .join(summary.run_id.as_str())
            .join("recovery-isolation.json");
        let text = std::fs::read_to_string(evidence).unwrap();
        assert!(text.contains(id.as_str()) && text.contains("NOT verified"));
        assert_eq!(
            before,
            std::fs::read(journal.root().join("manifest.json")).unwrap()
        );
        assert!(journal.root().join("actors/alice").is_dir());
        assert!(FileJournal::refuse_pending(journal.root().parent().unwrap()).is_err());
        assert!(
            crate::recovery_abandon::gate(
                &journal.snapshot(),
                crate::recovery_store::AbandonReason::StackDestroyed
            )
            .is_err()
        );
    }
}

#[tokio::test]
async fn isolation_requires_exact_ids_stopped_storage_and_current_regtest() {
    let world = world(false);
    let journal = legacy(&world);
    let id = journal.snapshot().run_id;
    let inspector = inspector(&world);
    for ids in [vec![], vec![RunId::new()], vec![id.clone(), id.clone()]] {
        assert!(
            authorization(&world, &ids, &inspector, "regtest")
                .await
                .is_err()
        );
    }
    for network in ["mainnet", "testnet", "signet", "unknown", ""] {
        assert!(
            authorization(&world, std::slice::from_ref(&id), &inspector, network)
                .await
                .is_err()
        );
    }
    let marker = journal.root().join("actors/alice/.mortsom-active-runtime");
    std::fs::write(&marker, "active").unwrap();
    assert!(
        authorization(&world, std::slice::from_ref(&id), &inspector, "regtest")
            .await
            .unwrap_err()
            .contains("shutdown")
    );
    assert!(marker.exists());
    std::fs::remove_file(marker).unwrap();
    std::fs::remove_dir(journal.root().join("actors/bob")).unwrap();
    assert!(
        authorization(&world, &[id], &inspector, "regtest")
            .await
            .is_err()
    );
}

#[tokio::test]
async fn isolation_refuses_mainnet_permission_canary_external_wallets_and_unowned_stacks() {
    let mut world = world(true);
    let journal = legacy(&world);
    let ids = [journal.snapshot().run_id];
    let mut inspector = inspector(&world);
    assert!(
        authorization(&world, &ids, &inspector, "regtest")
            .await
            .is_err()
    );
    world.config.run.allow_mainnet = false;
    world.config.run.lane = mortsom_common::Lane::Canary;
    assert!(
        authorization(&world, &ids, &inspector, "regtest")
            .await
            .is_err()
    );
    world.config.run.lane = mortsom_common::Lane::Local;
    let original = world.config.wallets.alice.clone();
    if let WalletConfig::Lnd { host, .. } = &mut world.config.wallets.alice {
        *host = "external:10009".into();
    }
    assert!(
        authorization(&world, &ids, &inspector, "regtest")
            .await
            .is_err()
    );
    world.config.wallets.alice = original;
    for project in [
        ProjectOwner::Absent,
        ProjectOwner::Unprovable("unreadable labels".into()),
        ProjectOwner::Directory("/different".into()),
    ] {
        inspector.0.project = project;
        assert!(
            authorization(&world, &ids, &inspector, "regtest")
                .await
                .is_err()
        );
    }
}

#[tokio::test]
async fn a_modern_or_matching_or_unreadable_journal_cannot_be_preserved() {
    let world = world(false);
    let journal = legacy(&world);
    let ids = [journal.snapshot().run_id];
    let inspector = inspector(&world);
    let path = journal.root().join("manifest.json");
    let mut manifest = journal.snapshot();
    for fingerprint in [
        stack().fingerprint().unwrap(),
        stack().legacy_fingerprint().unwrap(),
    ] {
        manifest.stack = fingerprint;
        std::fs::write(&path, serde_json::to_vec(&manifest).unwrap()).unwrap();
        assert!(
            authorization(&world, &ids, &inspector, "regtest")
                .await
                .unwrap_err()
                .contains("belongs to the selected stack")
        );
    }
    manifest.stack = journal.snapshot().stack;
    manifest.environment = Some(crate::recovery_outcome::tests::regtest());
    std::fs::write(&path, serde_json::to_vec(&manifest).unwrap()).unwrap();
    assert!(
        authorization(&world, &ids, &inspector, "regtest")
            .await
            .unwrap_err()
            .contains("not an unresolved legacy")
    );
    std::fs::write(&path, "{").unwrap();
    assert!(
        authorization(&world, &ids, &inspector, "regtest")
            .await
            .is_err()
    );
}

#[tokio::test]
async fn a_new_pending_journal_or_changed_network_still_blocks_the_next_scenario() {
    mortsom_scenarios::link();
    for changed_network in [false, true] {
        let world = world(false);
        let journal = legacy(&world);
        let mut options = opts_in(&world, &["cancel_before_taken"]);
        options.stack_dir = Some(world.dir.path().to_path_buf());
        options.recovery_isolation = Some(
            authorization(
                &world,
                &[journal.snapshot().run_id],
                &inspector(&world),
                "regtest",
            )
            .await
            .unwrap(),
        );
        if !changed_network {
            let mut manifest = journal.snapshot();
            manifest.run_id = RunId::new();
            FileJournal::create(journal.root().parent().unwrap(), manifest).unwrap();
        }
        let network = if changed_network {
            "mainnet"
        } else {
            "regtest"
        };
        let summary = run_on_nodes(
            &world,
            Arc::new(FakeAdapter::full()),
            options,
            false,
            &FakeObserver::default(),
            network,
        )
        .await
        .unwrap();
        assert!(!summary.passed);
        assert!(
            summary.actors.is_empty(),
            "no client may start before the repeated gates"
        );
        assert!(summary.results.iter().all(|result| result.attempts == 0));
    }
}

fn malformed_digests() -> Vec<String> {
    vec![
        String::new(),
        "a".repeat(63),
        "a".repeat(65),
        "g".repeat(64),
        "A".repeat(64),
        format!("{} ", "a".repeat(63)),
        "é".repeat(32),
    ]
}

#[test]
fn malformed_fingerprints_are_errors_not_stack_inequality() {
    let current = stack();
    for valid in [
        current.legacy_fingerprint().unwrap(),
        current.fingerprint().unwrap(),
    ] {
        assert!(current.matches_recorded(&valid).unwrap());
        for digest in malformed_digests() {
            let malformed = crate::stack::StackFingerprint {
                digest,
                ..valid.clone()
            };
            assert!(
                current.matches_recorded(&malformed).is_err(),
                "{malformed:?}"
            );
        }
        let mut changed = current.clone();
        changed.daemon_pubkey = "different".into();
        assert!(!changed.matches_recorded(&valid).unwrap());
        let unknown = crate::stack::StackFingerprint {
            algorithm: "unknown".into(),
            ..valid
        };
        assert!(current.matches_recorded(&unknown).is_err());
    }
}

#[tokio::test]
async fn malformed_fingerprints_refuse_journal_loading_and_isolation_without_mutation() {
    let world = world(false);
    let journal = legacy(&world);
    let ids = [journal.snapshot().run_id];
    let path = journal.root().join("manifest.json");
    for algorithm in [
        "sha256-mortsom-stack-v1",
        "sha256-mortsom-stack-v2",
        "unknown",
    ] {
        for digest in malformed_digests().into_iter().chain(["a".repeat(64)]) {
            if algorithm != "unknown" && digest == "a".repeat(64) {
                continue;
            }
            let mut manifest = journal.snapshot();
            manifest.stack = crate::stack::StackFingerprint {
                algorithm: algorithm.into(),
                digest,
            };
            let bytes = serde_json::to_vec(&manifest).unwrap();
            std::fs::write(&path, &bytes).unwrap();
            assert!(
                FileJournal::open(journal.root()).is_err(),
                "{:?}",
                manifest.stack
            );
            assert!(
                authorization(&world, &ids, &inspector(&world), "regtest")
                    .await
                    .unwrap_err()
                    .contains("stack fingerprint")
            );
            assert_eq!(std::fs::read(&path).unwrap(), bytes);
            for actor in Actor::ALL {
                assert!(journal.root().join("actors").join(actor.as_str()).is_dir());
            }
        }
    }
}
