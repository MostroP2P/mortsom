use super::*;

#[tokio::test]
async fn happy_sell_passes_end_to_end_with_evidence_and_no_open_orders() {
    mortsom_scenarios::link();
    let world = world(true);
    let adapter = Arc::new(FakeAdapter::full());

    let summary = run(
        &world,
        adapter.clone(),
        opts_in(&world, &["happy_sell"]),
        true,
    )
    .await
    .unwrap();

    assert!(
        summary.passed,
        "{}\n{:?}",
        summary.checks.render(),
        summary.results
    );
    assert_eq!(summary.results.len(), 1);
    assert!(summary.results[0].outcome.is_passed());
    assert!(summary.mainnet_cleared);
    let cleanup = summary.cleanup.as_ref().unwrap();
    assert!(cleanup.is_clean());
    assert!(
        adapter.cancelled.lock().unwrap().is_empty(),
        "a completed order is not cancelled"
    );
    let budget = summary.budget.unwrap();
    assert_eq!(budget.settled_sats, 1000);
    let run_dir = world
        .dir
        .path()
        .join("artifacts")
        .join(summary.run_id.as_str());
    for name in [
        files::SUMMARY,
        files::TIMELINE,
        files::TIMELINE_JSON,
        files::RELAY_EVENTS,
        files::WALLET_PAYMENTS,
        files::CLEANUP,
        files::ENVIRONMENT,
    ] {
        assert!(run_dir.join(name).is_file(), "{name}");
    }
    let timeline = std::fs::read_to_string(run_dir.join(files::TIMELINE)).unwrap();
    assert!(timeline.contains("created Sell order o1"));
    assert!(timeline.contains("relay: order o1 is completed"));
    assert!(timeline.contains("wallet:"));
    let everything = walk_text(&run_dir);
    assert!(
        !everything.contains(INVOICE),
        "invoice text leaked into artifacts"
    );
    assert!(!everything.contains("secret=71a8"));
}

/// Value moves only with clearance: on mainnet the double opt-in, on nodes
/// Mortsom holds a network every node reports as regtest. A node on any
/// other network stops the run before a client starts.
#[tokio::test]
async fn value_moving_scenario_is_refused_without_mainnet_clearance() {
    mortsom_scenarios::link();
    let no_opt_in = world(false);
    let summary = run_on_nodes(
        &no_opt_in,
        Arc::new(FakeAdapter::full()),
        opts_in(&no_opt_in, &["happy_sell"]),
        true,
        &FakeObserver::default(),
        "mainnet",
    )
    .await
    .unwrap();
    assert!(!summary.passed);
    assert!(summary.actors.is_empty(), "no client started");
    assert!(
        summary
            .checks
            .checks
            .iter()
            .any(|check| check.name == "regtest-clearance" && check.status == CheckStatus::Fail),
        "{}",
        summary.checks.render()
    );

    let opted_in = world(true);
    let err = run(
        &opted_in,
        Arc::new(FakeAdapter::full()),
        opts_in(&opted_in, &["happy_sell"]),
        false,
    )
    .await
    .unwrap_err();
    assert!(err.contains(MAINNET_CONFIRM_ENV), "{err}");
}

#[tokio::test]
async fn unknown_scenario_and_missing_capability_are_explicit() {
    mortsom_scenarios::link();
    let world = world(true);
    let err = run(
        &world,
        Arc::new(FakeAdapter::full()),
        opts_in(&world, &["nope"]),
        true,
    )
    .await
    .unwrap_err();
    assert!(err.contains("unknown scenario"));

    let mut limited = FakeAdapter::full();
    limited.capabilities = CapabilitySet::new([Capability::Onboarding]);
    let summary = run(
        &world,
        Arc::new(limited),
        opts_in(&world, &["happy_sell"]),
        true,
    )
    .await
    .unwrap();
    assert!(summary.passed, "skipped with reason still passes the run");
    assert!(
        matches!(&summary.results[0].outcome, Outcome::Skipped { reason } if reason.contains("lacks"))
    );
    assert!(
        summary.actors.is_empty(),
        "nothing was started for a fully gated run"
    );
}

/// What a scenario needs from the app depends on the wallet the run gave
/// it. On a node Mortsom holds the app is walletless by design: gating on
/// its NWC support skips a profile over a capability the run never
/// touches, and not gating on the manual pay and invoice controls starts a
/// profile that cannot drive them and lets it fail mid-trade instead of
/// being skipped with a reason.
#[test]
fn the_gate_asks_for_the_controls_the_run_will_actually_drive() {
    mortsom_scenarios::link();
    let happy = mortsom_scenario_kit::registry::find("happy_sell").unwrap();
    let reason = |results: &[ScenarioResult]| match &results[0].outcome {
        Outcome::Skipped { reason } => reason.clone(),
        other => panic!("expected a skip, got {other:?}"),
    };

    // The app pays: it must be able to.
    let no_nwc = without(Capability::Nwc);
    let (runnable, skipped) = gate_capabilities(&no_nwc, &[happy], ManualPayments::NONE);
    assert!(runnable.is_empty());
    assert!(reason(&skipped).contains("nwc"), "{}", reason(&skipped));

    // Mortsom pays for both: the app's own wallet is never reached.
    let (runnable, skipped) = gate_capabilities(&no_nwc, &[happy], ManualPayments::BOTH);
    assert_eq!(runnable.len(), 1, "{skipped:?}");
    assert!(skipped.is_empty());

    // But the controls Mortsom drives by hand must exist.
    let no_manual = without(Capability::ExternalWallet);
    let (runnable, skipped) = gate_capabilities(&no_manual, &[happy], ManualPayments::BOTH);
    assert!(runnable.is_empty());
    assert!(
        reason(&skipped).contains("external-wallet"),
        "{}",
        reason(&skipped)
    );
}

/// `[wallets]` lets each actor name its own backend, and a trade with one
/// node and one connected wallet still reaches the app's wallet on the
/// half that pays for itself. Reading the run as manual because *either*
/// actor is would let a profile without NWC start and fail mid-trade --
/// the false start the gate exists to prevent.
#[test]
fn a_mixed_run_still_gates_on_the_wallet_the_other_actor_pays_with() {
    mortsom_scenarios::link();
    let happy = mortsom_scenario_kit::registry::find("happy_sell").unwrap();
    let alice_on_a_node = ManualPayments {
        alice: true,
        bob: false,
    };

    let (runnable, skipped) =
        gate_capabilities(&without(Capability::Nwc), &[happy], alice_on_a_node);

    assert!(runnable.is_empty(), "bob's app still pays through NWC");
    match &skipped[0].outcome {
        Outcome::Skipped { reason } => assert!(reason.contains("nwc"), "{reason}"),
        other => panic!("expected a skip, got {other:?}"),
    }
}

/// A solver is registered with the daemon's own key, which only a
/// Mortsom-owned stack holds: without one, a scenario that needs a solver
/// is skipped with the reason, and one that does not is untouched.
#[test]
fn a_scenario_that_needs_a_solver_is_skipped_without_an_owned_stack() {
    mortsom_scenarios::link();
    let settled = mortsom_scenario_kit::registry::find("dispute_settled_by_solver").unwrap();
    let released = mortsom_scenario_kit::registry::find("dispute_released_by_seller").unwrap();

    let (runnable, skipped) = gate_solver("v2", vec![settled, released], false);

    assert_eq!(
        runnable.iter().map(|d| d.name).collect::<Vec<_>>(),
        ["dispute_released_by_seller"]
    );
    match &skipped[0].outcome {
        Outcome::Skipped { reason } => assert!(reason.contains("dispute solver"), "{reason}"),
        other => panic!("expected a skip, got {other:?}"),
    }

    let (runnable, skipped) = gate_solver("v2", vec![settled, released], true);
    assert_eq!(runnable.len(), 2);
    assert!(skipped.is_empty());
}

/// A scenario whose subject is the connected wallet -- cancelling from the
/// pay screen instead of paying through it -- cannot be exercised by a run
/// that pays from a node Mortsom holds: the seller never opens that
/// screen. Rewriting its requirements onto the manual controls would let
/// it report a pass for a path the run never took.
#[test]
fn a_scenario_about_the_connected_wallet_is_skipped_when_mortsom_pays_by_hand() {
    mortsom_scenarios::link();
    let cancel = mortsom_scenario_kit::registry::find("nwc_cancel_payment").unwrap();
    let full = FakeAdapter::full();

    let (runnable, skipped) = gate_capabilities(&full, &[cancel], ManualPayments::BOTH);

    assert!(runnable.is_empty(), "the NWC path is not reachable here");
    match &skipped[0].outcome {
        Outcome::Skipped { reason } => assert!(
            reason.contains("wallet the app is connected to"),
            "{reason}"
        ),
        other => panic!("expected a skip, got {other:?}"),
    }

    // On the wallet it is about, it runs.
    let (runnable, skipped) = gate_capabilities(&full, &[cancel], ManualPayments::NONE);
    assert_eq!(runnable.len(), 1, "{skipped:?}");
}

/// An adapter that supports everything but `dropped`.
fn without(dropped: Capability) -> FakeAdapter {
    let mut adapter = FakeAdapter::full();
    adapter.capabilities = CapabilitySet::new(
        FakeAdapter::full()
            .capabilities
            .iter()
            .filter(|c| *c != dropped),
    );
    adapter
}

/// The runbook's first instruction for a failed run is to open the actor's
/// last screen. Every checkpoint in the scenario kit sits on the success
/// path, so a failing action returned before recording one and left nothing
/// to open -- on exactly the run where it matters.
#[tokio::test]
async fn a_failed_scenario_leaves_the_screen_that_failed() {
    mortsom_scenarios::link();
    let world = world(true);
    let mut adapter = FakeAdapter::full();
    adapter.fail_release = true;

    let summary = run(
        &world,
        Arc::new(adapter),
        opts_in(&world, &["happy_sell"]),
        true,
    )
    .await
    .unwrap();

    assert!(!summary.passed);
    let run_dir = world
        .dir
        .path()
        .join("artifacts")
        .join(summary.run_id.as_str());
    for actor in ["alice", "bob"] {
        let steps = std::fs::read_dir(run_dir.join(actor).join("steps"))
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        // Step names are sanitized for the filesystem, so the scenario
        // reads `happy-sell` here.
        assert!(
            steps
                .iter()
                .any(|f| f.contains("happy-sell-failed") && f.ends_with("screen.png")),
            "{actor} kept no screen from the failure: {steps:?}"
        );
        assert!(
            steps
                .iter()
                .any(|f| f.contains("happy-sell-failed") && f.ends_with("ui.xml")),
            "{actor} kept no hierarchy from the failure: {steps:?}"
        );
    }
    // And the timeline points at it, because that is what a reader opens
    // first.
    let timeline = std::fs::read_to_string(run_dir.join(files::TIMELINE)).unwrap_or_default();
    assert!(
        timeline.contains("screen when happy_sell failed"),
        "{timeline}"
    );
}

/// Diagnostics may never postpone cleanup. When the driver is what broke,
/// each screenshot and hierarchy waits out its own 60-second HTTP timeout,
/// so capturing two actors one after the other would hold the run -- and
/// the sats it still has open -- for minutes, and would photograph the
/// second phone long after the moment it is meant to show.
#[tokio::test(start_paused = true)]
async fn a_silent_driver_cannot_hold_the_run_open_while_it_is_photographed() {
    mortsom_scenarios::link();
    let world = world(true);
    let mut adapter = FakeAdapter::full();
    adapter.fail_release = true;
    adapter.silence_driver_on_failure = true;

    let started = tokio::time::Instant::now();
    let summary = run(
        &world,
        Arc::new(adapter),
        opts_in(&world, &["happy_sell"]),
        true,
    )
    .await
    .unwrap();
    let spent = started.elapsed();

    assert!(!summary.passed);
    // Four unbounded requests (a screen and a hierarchy each for Alice and
    // Bob) would be four minutes; both actors under one deadline is
    // seconds.
    assert!(
        spent < Duration::from_secs(60),
        "the failure capture held the run for {spent:?}"
    );
}

#[tokio::test]
async fn infrastructure_failure_after_payment_leaves_cleanup_to_cancel_the_open_order() {
    mortsom_scenarios::link();
    let world = world(true);
    let mut adapter = FakeAdapter::full();
    adapter.fail_release = true;
    let adapter = Arc::new(adapter);

    let summary = run(
        &world,
        adapter.clone(),
        opts_in(&world, &["happy_sell"]),
        true,
    )
    .await
    .unwrap();

    assert!(!summary.passed);
    let result = &summary.results[0];
    assert_eq!(result.attempts, 1, "a scenario runs exactly once");
    assert!(matches!(
        &result.outcome,
        Outcome::Failed {
            category: mortsom_common::FailureCategory::TestInfrastructure,
            ..
        }
    ));
    assert!(
        adapter.cancelled.lock().unwrap().contains(&"o1".to_owned()),
        "cleanup cancelled the open order"
    );
    assert!(summary.cleanup.as_ref().unwrap().is_clean());
    assert_eq!(
        summary.budget.unwrap().settled_sats,
        1000,
        "exactly one payment"
    );
}

fn noop(_ctx: &Context) -> ScenarioFuture<'_> {
    Box::pin(async { Ok(()) })
}

fn flaky_infra(ctx: &Context) -> ScenarioFuture<'_> {
    Box::pin(async move {
        // Fails with an infrastructure category on the first attempt only.
        let attempts = ctx
            .timeline
            .entries()
            .iter()
            .filter(|e| e.summary.contains("attempt") && e.summary.contains("started"))
            .count();
        if attempts <= 1 {
            Err(mortsom_scenario_kit::context::ScenarioError::new(
                mortsom_common::FailureCategory::TestInfrastructure,
                "session lost",
            ))
        } else {
            Ok(())
        }
    })
}

inventory::submit! {
    ScenarioDef {
        name: "zz_runner_flaky_infra",
        tags: &["runner-selftest"],
        requires: &[],
        timeout_secs: 5,
        moves_value: false,
        needs_app_wallet: false,
        needs_solver: false,
        needs: &[],
        run: flaky_infra,
    }
}

/// A client keeps its actor storage across attempts, so an infrastructure
/// failure is never retried inside one run: a retry would reopen the trade
/// the failed attempt started rather than begin a fresh one.
#[tokio::test]
async fn infrastructure_failure_is_not_retried_inside_one_run() {
    let world = world(false);
    let summary = run(
        &world,
        Arc::new(FakeAdapter::full()),
        opts_in(&world, &["zz_runner_flaky_infra"]),
        false,
    )
    .await
    .unwrap();
    assert!(!summary.passed, "{:?}", summary.results);
    assert_eq!(summary.results[0].attempts, 1);
}

inventory::submit! {
    ScenarioDef {
        name: "zz_runner_noop",
        tags: &["runner-selftest"],
        requires: &[],
        timeout_secs: 5,
        moves_value: false,
        needs_app_wallet: false,
        needs_solver: false,
        needs: &[],
        run: noop,
    }
}

#[tokio::test]
async fn non_value_scenarios_run_without_clearance() {
    let world = world(false);
    let summary = run(
        &world,
        Arc::new(FakeAdapter::full()),
        opts_in(&world, &["zz_runner_noop"]),
        false,
    )
    .await
    .unwrap();
    assert!(summary.passed, "{}", summary.checks.render());
    assert!(!summary.mainnet_cleared);
    assert_eq!(summary.results[0].attempts, 1);
}

fn walk_text(dir: &Path) -> String {
    let mut out = String::new();
    for entry in std::fs::read_dir(dir).unwrap().flatten() {
        let path = entry.path();
        if path.is_dir() {
            out.push_str(&walk_text(&path));
        } else if path.extension().is_some_and(|e| e != "png") {
            out.push_str(&std::fs::read_to_string(&path).unwrap_or_default());
        }
    }
    out
}
