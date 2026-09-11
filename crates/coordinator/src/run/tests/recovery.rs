//! Coordinator restart tests exercise the durable journal and public UI edges.
use super::super::recovery as recovery_impl;
use super::*;
use crate::recovery_store::{FileJournal, RecoveryManifest};
use mortsom_common::Actor;
use mortsom_desktop_driver::Platform;
use mortsom_scenario_kit::journal::RecoveryEvent;

pub(super) struct RetainedUi {
    identity: String,
    calls: Mutex<Vec<&'static str>>,
}
#[async_trait]
impl UiSession for RetainedUi {
    async fn exists(&self, _: &str) -> Result<bool, UiError> {
        unreachable!()
    }
    async fn wait_for(&self, _: &str, _: Duration) -> Result<(), UiError> {
        unreachable!()
    }
    async fn wait_gone(&self, _: &str, _: Duration) -> Result<(), UiError> {
        unreachable!()
    }
    async fn tap(&self, _: &str) -> Result<(), UiError> {
        panic!("recovery must not generate an identity")
    }
    async fn type_text(&self, _: &str, _: &str) -> Result<(), UiError> {
        unreachable!()
    }
    async fn text_of(&self, id: &str) -> Result<String, UiError> {
        assert_eq!(id, "identity");
        self.calls.lock().unwrap().push("read identity");
        Ok(self.identity.clone())
    }
    async fn is_enabled(&self, _: &str) -> Result<bool, UiError> {
        unreachable!()
    }
    async fn label_of(&self, _: &str) -> Result<String, UiError> {
        unreachable!()
    }
    async fn screenshot_png(&self) -> Result<Vec<u8>, UiError> {
        Ok(vec![])
    }
    async fn page_source(&self) -> Result<String, UiError> {
        Ok("<fixture/>".into())
    }
    async fn background(&self, _: u64) -> Result<(), UiError> {
        unreachable!()
    }
    async fn terminate_app(&self) -> Result<(), UiError> {
        unreachable!()
    }
    async fn activate_app(&self) -> Result<(), UiError> {
        self.calls.lock().unwrap().push("activate");
        Ok(())
    }
    async fn clear_app(&self) -> Result<(), UiError> {
        panic!("retained identity must never be cleared")
    }
    async fn set_connectivity(&self, _: bool) -> Result<(), UiError> {
        unreachable!()
    }
    async fn set_orientation(&self, _: bool) -> Result<(), UiError> {
        unreachable!()
    }
    async fn hide_keyboard(&self) -> Result<(), UiError> {
        unreachable!()
    }
    async fn end(&self) -> Result<(), UiError> {
        self.calls.lock().unwrap().push("end");
        Ok(())
    }
}

fn stack() -> crate::stack::StackSnapshot {
    serde_json::from_value(serde_json::json!({
        "daemon_pubkey":HEX,"relay_url":"ws://127.0.0.1:7000","wallet_endpoints":{},
        "daemon_image":{"image_id":format!("sha256:{}", "a".repeat(64)),
            "reference":"mostrop2p/mostro:latest","repository_digest":null},
        "declared_limits":{"order_expiration_secs":300,"hold_invoice_expiry_secs":120,
            "buyer_invoice_deadline_secs":120,"payout_invoice_min_expiry_secs":300}
    }))
    .unwrap()
}

pub(super) fn saved_run(fixture: &World) -> Arc<FileJournal> {
    saved_run_for(fixture, Actor::Alice)
}

fn saved_run_for(fixture: &World, maker: Actor) -> Arc<FileJournal> {
    let source = fixture.dir.path().join("source");
    std::fs::create_dir(&source).unwrap();
    std::fs::write(source.join("mostro"), b"retained app fixture").unwrap();
    let bundle = crate::fingerprint::CapturedBundle::capture(&source).unwrap();
    let identities = BTreeMap::from([
        ("alice".into(), HEX.into()),
        ("bob".into(), Keys::generate().public_key().to_hex()),
    ]);
    let journal = FileJournal::create(
        &fixture.dir.path().join("recovery"),
        RecoveryManifest {
            run_id: RunId::new(),
            stack: stack().fingerprint().unwrap(),
            platform: Platform::Linux,
            bundle: bundle.fingerprint().clone(),
            build: crate::provenance::BuildProvenance::prebuilt(),
            executable: Some("mostro".into()),
            identities: identities.clone(),
            origin: None,
            events: vec![
                RecoveryEvent::SubmissionStarted {
                    actor: maker,
                    known_orders: vec![],
                },
                RecoveryEvent::OrderFound {
                    actor: maker,
                    order: OrderReference::new("old-order"),
                },
            ],
            closed: false,
            resolution_verified: false,
        },
    )
    .unwrap();
    crate::fingerprint::CapturedBundle::capture_persistent(
        bundle.root(),
        &journal.root().join("bundle"),
    )
    .unwrap();
    for (actor, identity) in identities {
        let path = journal.root().join("actors").join(actor);
        crate::artifacts::persist::create_directory(&path).unwrap();
        // Fake public UI persistence, never application-private storage access.
        std::fs::write(path.join("public-identity"), identity).unwrap();
    }
    let root = journal.root().to_path_buf();
    drop(journal);
    Arc::new(FileJournal::open(&root).unwrap())
}

pub(super) fn options(journal: Arc<FileJournal>) -> RunOptions {
    let saved = journal.snapshot();
    let bundle = Arc::new(
        crate::fingerprint::CapturedBundle::resume(&journal.root().join("bundle"), saved.bundle)
            .unwrap(),
    );
    RunOptions {
        desktop: DesktopOptions {
            recovery: Some(journal),
            resume: true,
            platform: Platform::Linux,
            artifact: bundle.root().join("mostro"),
            bundle,
            provenance: saved.build,
            xvfb: "unused".into(),
            browser: "unused".into(),
            chromedriver: "unused".into(),
        },
        stack: Some(stack()),
        scenarios: Vec::new(),
        tag: None,
    }
}

pub(super) fn no_wallets<'a>(
    _: &'a str,
    _: &'a mortsom_environment::WalletConfig,
) -> futures::future::BoxFuture<'a, Result<WalletHandles, String>> {
    Box::pin(async { panic!("unfunded recovery must not construct or mutate wallets") })
}

async fn attempt(
    fixture: &World,
    adapter: Arc<FakeAdapter>,
    journal: Arc<FileJournal>,
) -> (RunSummary, Vec<Arc<RetainedUi>>) {
    attempt_with_wallet(fixture, adapter, journal, None).await
}

async fn attempt_with_wallet(
    fixture: &World,
    adapter: Arc<FakeAdapter>,
    journal: Arc<FileJournal>,
    wallet: Option<(String, Arc<RecoveryWallet>)>,
) -> (RunSummary, Vec<Arc<RetainedUi>>) {
    attempt_with_wallet_and_book(fixture, adapter, journal, wallet, &["canceled", "success"]).await
}

async fn attempt_with_wallet_and_book(
    fixture: &World,
    adapter: Arc<FakeAdapter>,
    journal: Arc<FileJournal>,
    wallet: Option<(String, Arc<RecoveryWallet>)>,
    book_statuses: &[&'static str],
) -> (RunSummary, Vec<Arc<RetainedUi>>) {
    let env = |_: &str| None;
    let deps = RunDeps {
        desktop_factory: &RealDesktopFactory,
        net: None,
        docker: Arc::new(FakeDocker::default()),
        probe: Arc::new(AlwaysReady),
        adapter: adapter.clone(),
        env: &env,
        wallets: &no_wallets,
        observer: &FakeObserver::default(),
    };
    let options = options(journal.clone());
    let (artifacts, mut summary) = report_fixture(fixture, &options);
    let mut world = super::super::World::new(
        &fixture.config,
        &options,
        &deps,
        &artifacts,
        SpendCaps::new(1000, 1000).unwrap(),
        None,
        false,
    );
    let sessions = attach_runtime_with_book(
        &mut world,
        &journal,
        options.desktop.bundle.clone(),
        book_statuses,
    );
    if let Some((label, wallet)) = wallet {
        world.wallets = Some(Arc::new(WalletObserver::new(vec![(
            label.clone(),
            wallet.clone(),
        )])));
        world.node_wallets.insert(label, wallet);
    }
    resume_identities(&world, &journal, &sessions, &mut summary).await;
    if summary.checks.is_healthy() {
        let result = world
            .execute(&fixture.config, &recovery_impl::CLEANUP, &mut summary)
            .await;
        summary.results.push(result);
    }
    world.tear_down(&mut summary).await;
    (summary, sessions)
}

pub(super) fn report_fixture(fixture: &World, options: &RunOptions) -> (RunArtifacts, RunSummary) {
    let artifacts = RunArtifacts::create(
        fixture.dir.path(),
        RunId::new(),
        mortsom_common::Redactor::new(),
    )
    .unwrap();
    let summary = super::super::report::initial_summary(
        &fixture.config,
        options,
        artifacts.run_id().clone(),
        "fake".into(),
        false,
    )
    .unwrap();
    (artifacts, summary)
}

pub(super) fn attach_runtime(
    world: &mut super::super::World<'_>,
    journal: &FileJournal,
    bundle: Arc<crate::fingerprint::CapturedBundle>,
) -> Vec<Arc<RetainedUi>> {
    attach_runtime_with_book(world, journal, bundle, &["canceled", "success"])
}

/// Like [`attach_runtime`], with the statuses the fixture daemon publishes
/// on the book for the saved order.
pub(super) fn attach_runtime_with_book(
    world: &mut super::super::World<'_>,
    journal: &FileJournal,
    bundle: Arc<crate::fingerprint::CapturedBundle>,
    book_statuses: &[&'static str],
) -> Vec<Arc<RetainedUi>> {
    let book_statuses: Vec<&'static str> = book_statuses.to_vec();
    let sessions: Vec<_> = Actor::ALL
        .into_iter()
        .map(|actor| {
            Arc::new(RetainedUi {
                identity: std::fs::read_to_string(
                    journal
                        .root()
                        .join("actors")
                        .join(actor.as_str())
                        .join("public-identity"),
                )
                .unwrap(),
                calls: Mutex::new(Vec::new()),
            })
        })
        .collect();
    world.farm = Some(Farm::Desktop {
        sessions: Actor::ALL
            .into_iter()
            .zip(sessions.iter())
            .map(|(actor, session)| (actor, session.clone() as Arc<dyn UiSession>))
            .collect(),
        server: None,
        bundle,
        app_version: None,
    });
    // The fixture daemon: it publishes `canceled` for the saved order, since
    // cleanup now believes the relay rather than a screen about cancellation.
    let daemon = Keys::generate();
    let observer = Arc::new(RelayObserver::detached(ObserverScope::daemon_only(
        daemon.public_key(),
    )));
    let feeder = Arc::clone(&observer);
    tokio::spawn(async move {
        for _ in 0..200 {
            for status in &book_statuses {
                let event = EventBuilder::new(Kind::Custom(38383), "")
                    .tags([
                        Tag::parse(["d", "old-order"]).unwrap(),
                        Tag::parse(["s", status]).unwrap(),
                    ])
                    .finalize(&daemon)
                    .unwrap();
                feeder.ingest("ws://fixture", &event);
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    });
    world.observer = Some(observer);
    world.wallets = Some(Arc::new(WalletObserver::new(Vec::new())));
    sessions
}

async fn resume_identities(
    world: &super::super::World<'_>,
    journal: &FileJournal,
    sessions: &[Arc<RetainedUi>],
    summary: &mut RunSummary,
) {
    for (actor, session) in Actor::ALL.into_iter().zip(sessions) {
        let device = DeviceContext {
            actor,
            ui: session.as_ref(),
            ui_timeout: Duration::from_millis(50),
        };
        match recovery_impl::resume_actor(world, &device, journal).await {
            Ok(actor) => summary.actors.push(actor),
            Err(error) => {
                summary
                    .checks
                    .push("recovery-identity", CheckStatus::Fail, error);
                break;
            }
        }
    }
}

#[tokio::test]
async fn reopened_run_uses_original_identities_and_cancels_the_saved_order_through_ui() {
    let fixture = world(false);
    let journal = saved_run(&fixture);
    let root = journal.root().to_path_buf();
    let adapter = Arc::new(FakeAdapter::full());
    adapter.set("old-order", OrderState::Pending);
    let (summary, sessions) = attempt(&fixture, adapter.clone(), journal).await;
    assert!(summary.checks.is_healthy(), "{:?}", summary.checks);
    assert!(summary.cleanup.as_ref().unwrap().is_clean());
    assert_eq!(
        summary.cleanup.as_ref().unwrap().entries[0].order.id,
        "old-order"
    );
    assert_eq!(*adapter.cancelled.lock().unwrap(), ["old-order"]);
    assert_eq!(*adapter.created.lock().unwrap(), 0);
    assert_eq!(
        *adapter.recovery_calls.lock().unwrap(),
        [
            "existing:alice",
            "configure:alice",
            "existing:bob",
            "configure:bob",
            "owned:alice"
        ]
    );
    for session in sessions {
        assert_eq!(
            *session.calls.lock().unwrap(),
            ["activate", "read identity", "end"]
        );
    }
    assert!(!root.join("actors").exists() && !root.join("bundle").exists());
    assert!(FileJournal::open(&root).unwrap().snapshot().closed);
}

#[tokio::test]
async fn changed_retained_identity_refuses_configuration_and_order_cleanup() {
    let fixture = world(false);
    let journal = saved_run(&fixture);
    std::fs::write(
        journal.root().join("actors/alice/public-identity"),
        "different identity",
    )
    .unwrap();
    let adapter = Arc::new(FakeAdapter::full());
    adapter.set("old-order", OrderState::Pending);
    let (summary, _) = attempt(&fixture, adapter.clone(), journal.clone()).await;
    assert!(
        summary
            .checks
            .checks
            .iter()
            .any(|check| check.detail.contains("identity mismatch"))
    );
    assert_eq!(*adapter.recovery_calls.lock().unwrap(), ["existing:alice"]);
    assert!(adapter.cancelled.lock().unwrap().is_empty());
    assert!(!journal.snapshot().closed && journal.root().join("actors").exists());
    // The screen at the moment of refusal is the only diagnostic a live
    // recovery leaves behind; it must be captured before the client closes.
    assert!(
        failed_recovery_steps(fixture.dir.path(), "alice")
            .iter()
            .any(|step| step.contains("recovery-identity-failed")),
        "no recovery-identity-failed evidence for alice"
    );
}

/// Step artifact names recorded for `actor` under any run in `root`.
fn failed_recovery_steps(root: &std::path::Path, actor: &str) -> Vec<String> {
    let mut names = Vec::new();
    for run in std::fs::read_dir(root).into_iter().flatten().flatten() {
        let steps = run.path().join(actor).join("steps");
        for entry in std::fs::read_dir(steps).into_iter().flatten().flatten() {
            names.push(entry.file_name().to_string_lossy().into_owned());
        }
    }
    names
}

#[tokio::test]
async fn failed_cleanup_keeps_profiles_until_a_new_coordinator_proves_resolution() {
    let fixture = world(false);
    let journal = saved_run(&fixture);
    let root = journal.root().to_path_buf();
    let mut adapter = FakeAdapter::full();
    adapter.stubborn = true;
    adapter.set("old-order", OrderState::Pending);
    let (failed, _) = attempt(&fixture, Arc::new(adapter), journal.clone()).await;
    assert!(!failed.cleanup.as_ref().unwrap().is_clean());
    assert!(!journal.snapshot().closed);
    assert!(
        root.join("actors/alice/public-identity").exists() && root.join("bundle/mostro").exists()
    );
    drop(journal);
    let reopened = Arc::new(FileJournal::open(&root).unwrap());
    let adapter = Arc::new(FakeAdapter::full());
    adapter.set("old-order", OrderState::Pending);
    let (success, _) = attempt(&fixture, adapter, reopened.clone()).await;
    assert!(success.cleanup.as_ref().unwrap().is_clean());
    assert!(success.checks.is_healthy(), "{:?}", success.checks);
    assert!(reopened.snapshot().closed && reopened.snapshot().resolution_verified);
    assert!(!root.join("actors").exists() && !root.join("bundle").exists());
}

#[test]
fn changed_stack_refuses_resume_before_environment_or_process_access() {
    let fixture = world(false);
    let journal = saved_run(&fixture);
    let mut options = options(journal.clone());
    options.stack.as_mut().unwrap().daemon_pubkey = Keys::generate().public_key().to_hex();
    let adapter = Arc::new(FakeAdapter::full());
    let env = |_: &str| panic!("stack mismatch must precede recovery environment access");
    let deps = RunDeps {
        desktop_factory: &RealDesktopFactory,
        net: None,
        docker: Arc::new(FakeDocker::default()),
        probe: Arc::new(AlwaysReady),
        adapter: adapter.clone(),
        env: &env,
        wallets: &no_wallets,
        observer: &FakeObserver::default(),
    };
    let error = recovery_impl::prepare(&mut options, &RunId::new(), &deps).unwrap_err();
    assert!(error.contains("stack identity changed"));
    assert!(adapter.recovery_calls.lock().unwrap().is_empty());
    assert!(!journal.snapshot().closed && journal.root().join("actors").exists());
}

struct RecoveryWallet {
    state: Mutex<PaymentState>,
    amount: u64,
    fee: u64,
    count: usize,
    calls: Mutex<Vec<String>>,
}
impl RecoveryWallet {
    fn new(state: PaymentState, amount: u64, fee: u64, count: usize) -> Arc<Self> {
        Arc::new(Self {
            state: Mutex::new(state),
            amount,
            fee,
            count,
            calls: Mutex::new(Vec::new()),
        })
    }
}
#[async_trait]
impl mortsom_observers::wallet::WalletReader for RecoveryWallet {
    async fn lookup(&self, hash: &str) -> Result<Option<(u64, u64, PaymentState)>, String> {
        assert_eq!(hash, "c".repeat(64));
        self.calls.lock().unwrap().push("lookup".into());
        Ok(Some((self.amount, self.fee, *self.state.lock().unwrap())))
    }
}
#[async_trait]
impl NodeWallet for RecoveryWallet {
    fn routing_fee_cap_sats(&self) -> u64 {
        10
    }
    async fn cancel_invoice(&self, hash: &str) -> Result<(), String> {
        assert_eq!(hash, "c".repeat(64));
        self.calls.lock().unwrap().push("cancel invoice".into());
        let mut state = self.state.lock().unwrap();
        if *state == PaymentState::Settled {
            return Err("settled invoice cannot be cancelled".into());
        }
        *state = PaymentState::Failed;
        Ok(())
    }
    async fn settled_record_count(&self, hash: &str, incoming: bool) -> Result<usize, String> {
        assert_eq!(hash, "c".repeat(64));
        self.calls
            .lock()
            .unwrap()
            .push(format!("count incoming={incoming}"));
        Ok(self.count)
    }
    async fn make_invoice(&self, _: u64, _: &str) -> Result<Secret, String> {
        panic!("recovery cannot generate a replacement invoice")
    }
    async fn start_payment(&self, _: &Secret) -> Result<(), String> {
        panic!("recovery cannot repay an existing order")
    }
}

async fn payment_attempt(
    state: OrderState,
    incoming: bool,
    wallet: Arc<RecoveryWallet>,
    maker: Actor,
) -> (RunSummary, Arc<FakeAdapter>) {
    payment_attempt_with(state, incoming, false, wallet, maker).await
}

async fn payment_attempt_with(
    state: OrderState,
    incoming: bool,
    superseded: bool,
    wallet: Arc<RecoveryWallet>,
    maker: Actor,
) -> (RunSummary, Arc<FakeAdapter>) {
    use mortsom_scenario_kit::journal::RecoveryJournal;
    let mut fixture = world(false);
    fixture.config.run.settle_timeout_secs = 1;
    let journal = saved_run_for(&fixture, maker);
    let label = if incoming { "bob" } else { "alice" };
    journal
        .record(RecoveryEvent::PaymentExpected {
            order_id: "old-order".into(),
            wallet: label.into(),
            hash: "c".repeat(64),
            incoming,
            amount_sats: 100,
        })
        .unwrap();
    if superseded {
        journal
            .record(RecoveryEvent::PaymentSuperseded {
                order_id: "old-order".into(),
                wallet: label.into(),
                hash: "c".repeat(64),
            })
            .unwrap();
    }
    let root = journal.root().to_path_buf();
    drop(journal);
    let journal = Arc::new(FileJournal::open(&root).unwrap());
    let mut adapter = FakeAdapter::full();
    if state == OrderState::PayoutPending {
        adapter.complete_after_observations = Some(2);
    }
    let adapter = Arc::new(adapter);
    adapter.set("old-order", state);
    let (summary, _) = attempt_with_wallet(
        &fixture,
        adapter.clone(),
        journal.clone(),
        Some((label.into(), wallet)),
    )
    .await;
    assert_eq!(journal.snapshot().closed, summary.checks.is_healthy());
    (summary, adapter)
}

/// The daemon tells only the buyer about the completed payout, so a seller
/// client that missed the public success event stays on payout pending. With
/// the daemon's success on the book and one settled record per hash, the
/// journal resolves and the lagging screen is recorded; without the daemon's
/// success, a lagging screen keeps the journal open.
#[tokio::test]
async fn a_client_stuck_on_payout_pending_resolves_only_with_the_daemons_success() {
    use mortsom_scenario_kit::journal::RecoveryJournal;
    for (book, healthy) in [
        (&["canceled", "success"][..], true),
        (&["canceled"][..], false),
    ] {
        let mut fixture = world(false);
        fixture.config.run.settle_timeout_secs = 1;
        let journal = saved_run_for(&fixture, Actor::Bob);
        journal
            .record(RecoveryEvent::PaymentExpected {
                order_id: "old-order".into(),
                wallet: "bob".into(),
                hash: "c".repeat(64),
                incoming: true,
                amount_sats: 100,
            })
            .unwrap();
        let root = journal.root().to_path_buf();
        drop(journal);
        let journal = Arc::new(FileJournal::open(&root).unwrap());
        let adapter = Arc::new(FakeAdapter::full());
        // Never completes on screen: no `complete_after_observations`.
        adapter.set("old-order", OrderState::PayoutPending);
        let wallet = RecoveryWallet::new(PaymentState::Settled, 100, 0, 1);
        let (summary, _) = attempt_with_wallet_and_book(
            &fixture,
            adapter,
            journal.clone(),
            Some(("bob".into(), wallet)),
            book,
        )
        .await;
        assert_eq!(
            summary.checks.is_healthy(),
            healthy,
            "book={book:?}: {:?}",
            summary.checks
        );
        assert_eq!(journal.snapshot().closed, healthy);
    }
}

/// A payout invoice the scenario withdrew must never have been paid, even
/// on a trade that completed on a replacement: the node is asked to cancel
/// it again, and only a failed invoice with no settled record is safe.
#[tokio::test]
async fn a_superseded_payout_is_safe_only_when_it_never_settled() {
    let wallet = RecoveryWallet::new(PaymentState::Failed, 100, 0, 0);
    let (summary, _) = payment_attempt_with(
        OrderState::Completed,
        true,
        true,
        wallet.clone(),
        Actor::Bob,
    )
    .await;
    assert!(!payment_failed(&summary), "{:?}", summary.checks);
    assert!(
        wallet
            .calls
            .lock()
            .unwrap()
            .contains(&"cancel invoice".to_owned()),
        "{:?}",
        wallet.calls.lock().unwrap()
    );

    let wallet = RecoveryWallet::new(PaymentState::Settled, 100, 0, 1);
    let (summary, _) =
        payment_attempt_with(OrderState::Completed, true, true, wallet, Actor::Bob).await;
    assert!(payment_failed(&summary), "{:?}", summary.checks);
}

fn payment_failed(summary: &RunSummary) -> bool {
    summary
        .checks
        .checks
        .iter()
        .any(|check| check.name == "recovery-payments" && check.status == CheckStatus::Fail)
}

#[tokio::test]
async fn outgoing_unknown_never_proves_a_cancelled_escrow_was_refunded() {
    let wallet = RecoveryWallet::new(PaymentState::Unknown, 100, 0, 0);
    let (summary, _) =
        payment_attempt(OrderState::Cancelled, false, wallet.clone(), Actor::Alice).await;
    assert!(payment_failed(&summary));
    assert!(
        wallet
            .calls
            .lock()
            .unwrap()
            .iter()
            .all(|call| call != "cancel invoice")
    );
}

#[tokio::test]
async fn cancelled_buy_uses_bob_as_owner_and_cancels_his_unpaid_invoice_before_observation() {
    let wallet = RecoveryWallet::new(PaymentState::Pending, 100, 0, 0);
    let (summary, adapter) =
        payment_attempt(OrderState::Cancelled, true, wallet.clone(), Actor::Bob).await;
    assert!(summary.checks.is_healthy(), "{:?}", summary.checks);
    assert_eq!(
        adapter.observed_actors.lock().unwrap().first(),
        Some(&Actor::Bob)
    );
    assert_eq!(
        *wallet.calls.lock().unwrap(),
        ["cancel invoice", "lookup", "count incoming=true"]
    );
}

#[tokio::test]
async fn settled_duplicate_record_or_wrong_amount_or_fee_keeps_recovery_open() {
    for (amount, fee, count) in [(100, 0, 2), (99, 0, 1), (100, 1, 1)] {
        let wallet = RecoveryWallet::new(PaymentState::Settled, amount, fee, count);
        let (summary, _) = payment_attempt(OrderState::Completed, true, wallet, Actor::Bob).await;
        assert!(
            payment_failed(&summary),
            "amount={amount}, fee={fee}, count={count}"
        );
    }
}

#[tokio::test]
async fn completed_trade_requires_exactly_one_matching_payout_and_both_actor_states() {
    let wallet = RecoveryWallet::new(PaymentState::Settled, 100, 0, 1);
    let (summary, adapter) =
        payment_attempt(OrderState::Completed, true, wallet.clone(), Actor::Bob).await;
    assert!(summary.checks.is_healthy(), "{:?}", summary.checks);
    let observations = adapter.observed_actors.lock().unwrap();
    assert_eq!(observations.first(), Some(&Actor::Bob));
    assert!(observations.ends_with(&[Actor::Alice, Actor::Bob]));
    assert_eq!(
        *wallet.calls.lock().unwrap(),
        ["lookup", "count incoming=true"]
    );
}

#[tokio::test]
async fn an_outgoing_fee_above_the_configured_cap_never_proves_safe_settlement() {
    let wallet = RecoveryWallet::new(PaymentState::Settled, 100, 11, 1);
    let (summary, _) = payment_attempt(OrderState::Completed, false, wallet, Actor::Alice).await;
    assert!(
        payment_failed(&summary),
        "outgoing fee exceeds the 10-sat wallet cap"
    );
}

#[tokio::test]
async fn resumed_payout_pending_waits_for_completion_without_attempting_cancellation() {
    let wallet = RecoveryWallet::new(PaymentState::Settled, 100, 0, 1);
    let (summary, adapter) =
        payment_attempt(OrderState::PayoutPending, true, wallet, Actor::Bob).await;
    assert!(summary.checks.is_healthy(), "{:?}", summary.checks);
    assert!(summary.cleanup.as_ref().unwrap().is_clean());
    assert!(adapter.cancelled.lock().unwrap().is_empty());
}
