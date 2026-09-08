//! Missing teardown evidence must fail acceptance and preserve recovery profiles.
use super::recovery::{attach_runtime, no_wallets, options, report_fixture, saved_run};
use super::*;

#[tokio::test]
async fn failed_teardown_evidence_retains_profiles_and_fails_acceptance() {
    for leaf in [
        files::RELAY_EVENTS,
        files::WALLET_PAYMENTS,
        files::TIMELINE,
        files::TIMELINE_JSON,
        "relay-container.log",
    ] {
        let (summary, retained) = teardown_with_evidence_failure(Some(leaf)).await;
        assert!(!summary.passed, "{leaf}: missing evidence accepted");
        assert!(retained, "{leaf}: recovery profiles removed");
        assert!(
            summary.checks.checks.iter().any(|check| {
                check.name == "teardown-evidence"
                    && check.status == CheckStatus::Fail
                    && check.detail.contains(leaf)
            }),
            "{leaf}: {}",
            summary.checks.render()
        );
        assert_eq!(summary.farm_teardown.len(), 2);
        assert!(summary.farm_teardown.iter().all(|item| item.ok));
        assert!(summary.relay_teardown.as_ref().unwrap().removed);
        assert!(summary.recovery.unwrap().retained);
    }
}

#[tokio::test]
async fn successful_teardown_evidence_allows_clean_profile_removal() {
    let (summary, retained) = teardown_with_evidence_failure(None).await;
    assert!(summary.passed, "{}", summary.checks.render());
    assert!(!retained);
    assert!(!summary.recovery.unwrap().retained);
}

async fn teardown_with_evidence_failure(leaf: Option<&str>) -> (RunSummary, bool) {
    let fixture = world(false);
    let journal = saved_run(&fixture);
    let options = options(journal.clone());
    let (artifacts, mut summary) = report_fixture(&fixture, &options);
    if let Some(leaf) = leaf {
        std::fs::create_dir(artifacts.file(leaf).unwrap()).unwrap();
    }
    let runner = FakeRunner::default();
    let ui = FakeUi(Flags::default());
    let env = |_: &str| None;
    let deps = RunDeps {
        desktop_factory: &RealDesktopFactory,
        net: None,
        runner: &runner,
        ui: &ui,
        docker: Arc::new(FakeDocker::default()),
        probe: Arc::new(AlwaysReady),
        adapter: Arc::new(FakeAdapter::full()),
        env: &env,
        wallets: &no_wallets,
        observer: &FakeObserver,
    };
    let mut world = super::super::World::new(
        &fixture.config,
        &options,
        &deps,
        &artifacts,
        SpendCaps::new(1000, 1000).unwrap(),
        None,
        false,
    );
    let _sessions = attach_runtime(
        &mut world,
        &journal,
        options.desktop.as_ref().unwrap().bundle.clone(),
    );
    attach_evidence_sources(&mut world, &fixture);
    world.tear_down(&mut summary).await;
    let _report = super::super::report::finish(&artifacts, &mut summary);
    (summary, journal.root().join("actors").exists())
}

fn attach_evidence_sources(world: &mut super::super::World<'_>, fixture: &World) {
    let keys = Keys::parse(NSEC).unwrap();
    let mut scope = ObserverScope::daemon_only(keys.public_key());
    scope.add_order("old-order");
    let observer = Arc::new(RelayObserver::detached(scope));
    let event = EventBuilder::new(Kind::Custom(38383), "")
        .tags([
            Tag::parse(["d", "old-order"]).unwrap(),
            Tag::parse(["s", "success"]).unwrap(),
        ])
        .finalize(&keys)
        .unwrap();
    observer.ingest("ws://fake", &event);
    assert_eq!(observer.snapshot().len(), 1);
    world.observer = Some(observer);
    world.relay = Some(RelayHandle {
        container_name: "evidence-fixture-relay".into(),
        host_port: 7000,
        data_dir: fixture.dir.path().join("relay-data"),
        image: "fixture".into(),
    });
}
