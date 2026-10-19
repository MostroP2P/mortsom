//! Phase 5 runs against the fakes: recovery and regression scenarios,
//! relay control, and honest cleanup.

use super::*;

/// Every Phase 5 scenario that moves no value runs end to end against the
/// fakes; the relay restart and the two relay replays are skipped, since the
/// stack relay is not the run's to restart or replay into (spec 8.1), and
/// the sweep closes what scenarios left open.
#[tokio::test]
async fn phase5_no_value_scenarios_run_against_fakes() {
    mortsom_scenarios::link();
    let world = world(false);
    let adapter = Arc::new(FakeAdapter::full());
    let names = [
        "restart_at_pending",
        "restart_at_taken",
        "background_at_taken",
        "network_loss_at_taken",
        "rotation_at_taken",
        "relay_restart_at_taken",
        "duplicate_events_at_taken",
        "stale_event_at_taken",
        "unauthorized_actions_at_taken",
        "cancel_before_taken",
        "taker_cancel_at_taken",
        "nwc_cancel_payment",
    ];
    let summary = run(&world, adapter.clone(), opts_in(&world, &names), false)
        .await
        .unwrap();

    let by_name: BTreeMap<&str, &Outcome> = summary
        .results
        .iter()
        .map(|r| (r.name.as_str(), &r.outcome))
        .collect();
    for name in names {
        let outcome = by_name[name];
        if name.ends_with("event_at_taken")
            || name.ends_with("events_at_taken")
            || name == "relay_restart_at_taken"
        {
            assert!(
                matches!(outcome, Outcome::Skipped { reason } if reason.contains("relay-faults")),
                "{name}: {outcome:?}"
            );
        } else if name == "nwc_cancel_payment" {
            // The app has no wallet of its own on a run whose nodes Mortsom
            // holds, so a scenario about that wallet is skipped, not faked.
            assert!(
                matches!(outcome, Outcome::Skipped { reason } if reason.contains("node Mortsom holds")),
                "{name}: {outcome:?}"
            );
        } else {
            assert!(outcome.is_passed(), "{name}: {outcome:?}");
        }
    }
    assert!(!summary.mainnet_cleared);
    assert_eq!(summary.budget.unwrap().settled_sats, 0);
    assert!(summary.cleanup.unwrap().is_clean());
    let run_dir = world
        .dir
        .path()
        .join("artifacts")
        .join(summary.run_id.as_str());
    let timeline = std::fs::read_to_string(run_dir.join(files::TIMELINE)).unwrap();
    assert!(timeline.contains("force-stopped and relaunched"));
    assert!(timeline.contains("no actor protocol message"));
    assert!(timeline.contains("offers [pay-invoice, cancel]"));
    assert!(
        adapter
            .cancelled
            .lock()
            .unwrap()
            .iter()
            // Fake ids count every order the run created; the three relay
            // scenarios are skipped and create none.
            .any(|id| id == "o8"),
        "the republished order of taker_cancel_at_taken is closed by the sweep: {:?}",
        adapter.cancelled.lock().unwrap()
    );
}

/// The value-moving Phase 5 scenarios complete their trades (or cancel
/// cooperatively) against the fakes under clearance; the fake app never
/// lets go of a taken order, so `hold_invoice_expires` reports a product
/// failure once its bounded wait ends instead of hanging.
#[tokio::test]
async fn phase5_value_scenarios_run_against_fakes() {
    mortsom_scenarios::link();
    let world = world(true);
    let adapter = Arc::new(FakeAdapter::full());
    let names = [
        "restart_at_active",
        "restart_at_fiat_sent",
        "restart_at_completed",
        "cooperative_cancel",
        "wallet_unavailable_then_pay",
        "hold_invoice_expires",
    ];
    let summary = run(&world, adapter.clone(), opts_in(&world, &names), true)
        .await
        .unwrap();

    let by_name: BTreeMap<&str, &Outcome> = summary
        .results
        .iter()
        .map(|r| (r.name.as_str(), &r.outcome))
        .collect();
    for name in names {
        let outcome = by_name[name];
        if name == "hold_invoice_expires" {
            assert!(
                matches!(outcome, Outcome::Failed { category: mortsom_common::FailureCategory::Product, message } if message.contains("still open")),
                "{name}: {outcome:?}"
            );
        } else {
            assert!(outcome.is_passed(), "{name}: {outcome:?}");
        }
    }
    assert_eq!(summary.budget.unwrap().settled_sats, 5 * 1000);
    assert!(summary.cleanup.unwrap().is_clean());
    assert_eq!(
        adapter.cancelled.lock().unwrap().len(),
        2,
        "cooperative cancel + sweep of the unpaid order"
    );
}

/// Cleanup believes the screen, not the tap: an order that stays open after
/// both sides cancelled is reported unresolved and the run does not pass.
#[tokio::test]
async fn cleanup_reports_an_order_that_stays_open_honestly() {
    mortsom_scenarios::link();
    let world = world(true);
    let mut adapter = FakeAdapter::full();
    adapter.fail_release = true;
    adapter.stubborn = true;
    let adapter = Arc::new(adapter);

    let summary = run(&world, adapter, opts_in(&world, &["happy_sell"]), true)
        .await
        .unwrap();

    assert!(!summary.passed);
    let cleanup = summary.cleanup.unwrap();
    assert_eq!(cleanup.unresolved, vec!["o1".to_owned()]);
    let entry = &cleanup.entries[0];
    assert!(
        matches!(&entry.result, Some(mortsom_scenario_kit::cleanup::CleanupResult::Failed { message }) if message.contains("still fiat-sent after both sides cancelled")),
        "{entry:?}"
    );
}

/// A funded order is cancelled when the daemon says so on the relay, not
/// when one screen shows the word. The live run of 2026-09-08 recorded a
/// hold-invoice order as cancelled from Alice's UI alone while the daemon had
/// only opened a cooperative cancel and still held her 1005 sats.
#[tokio::test]
async fn cleanup_needs_the_relay_to_confirm_a_funded_cancellation() {
    mortsom_scenarios::link();
    let world = world(true);
    let mut adapter = FakeAdapter::full();
    adapter.fail_release = true;
    adapter.ui_claims_cancelled = true;
    let adapter = Arc::new(adapter);
    let observer = FakeObserver {
        publish_closed: false,
        closed_gate: None,
    };

    let summary = run_with_observer(
        &world,
        adapter,
        opts_in(&world, &["happy_sell"]),
        true,
        &observer,
    )
    .await
    .unwrap();

    assert!(!summary.passed);
    let cleanup = summary.cleanup.unwrap();
    assert_eq!(cleanup.unresolved, vec!["o1".to_owned()]);
    let entry = &cleanup.entries[0];
    assert!(
        matches!(&entry.result, Some(mortsom_scenario_kit::cleanup::CleanupResult::Failed { message }) if message.contains("relay never published canceled")),
        "{entry:?}"
    );
}

/// The maker's screen saying "cancelled" is not the daemon closing a funded
/// order. When the relay does not confirm it, cleanup asks the counterpart to
/// cancel as well, and only the daemon's `canceled` closes the entry.
#[tokio::test]
async fn cleanup_finishes_a_cooperative_cancel_the_maker_screen_called_done() {
    mortsom_scenarios::link();
    let world = world(true);
    let mut adapter = FakeAdapter::full();
    adapter.fail_release = true;
    adapter.maker_screen_lies = true;
    let adapter = Arc::new(adapter);
    let observer = FakeObserver {
        publish_closed: true,
        closed_gate: Some(Arc::new(Mutex::new(Vec::new()))),
    };
    let gate = observer.closed_gate.clone().unwrap();
    // The fake daemon publishes `canceled` only once Bob's cancel lands.
    let watched = adapter.clone();
    tokio::spawn(async move {
        loop {
            let cancelled = watched.cancelled.lock().unwrap().clone();
            *gate.lock().unwrap() = cancelled;
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    });

    let summary = run_with_observer(
        &world,
        adapter.clone(),
        opts_in(&world, &["happy_sell"]),
        true,
        &observer,
    )
    .await
    .unwrap();

    assert!(
        !summary.passed,
        "release failed; the scenario itself must not pass"
    );
    let cleanup = summary.cleanup.unwrap();
    assert!(cleanup.is_clean(), "{cleanup:?}");
    assert_eq!(*adapter.cancelled.lock().unwrap(), ["o1".to_owned()]);
}

/// The maker's screen going unreadable after her cancel request is not the
/// end of cleanup. The live run of 2026-10-15 left a funded order active:
/// Bob had asked to cancel, his detail no longer exposed `order.id`, and
/// cleanup gave up before asking Alice to accept, so `stack down` refused.
/// While the relay shows the order open, the counterpart cancels too, and
/// the daemon's own `canceled` closes the entry.
#[tokio::test]
async fn cleanup_asks_the_counterpart_when_the_maker_screen_cannot_be_read() {
    mortsom_scenarios::link();
    let world = world(true);
    let mut adapter = FakeAdapter::full();
    adapter.fail_release = true;
    adapter.maker_screen_unreadable = true;
    let adapter = Arc::new(adapter);
    let observer = FakeObserver {
        publish_closed: true,
        closed_gate: Some(Arc::new(Mutex::new(Vec::new()))),
    };
    let gate = observer.closed_gate.clone().unwrap();
    // The fake daemon publishes `canceled` only once Bob's cancel lands.
    let watched = adapter.clone();
    tokio::spawn(async move {
        loop {
            let cancelled = watched.cancelled.lock().unwrap().clone();
            *gate.lock().unwrap() = cancelled;
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    });

    let summary = run_with_observer(
        &world,
        adapter.clone(),
        opts_in(&world, &["happy_sell"]),
        true,
        &observer,
    )
    .await
    .unwrap();

    assert!(!summary.passed);
    let cleanup = summary.cleanup.unwrap();
    assert!(cleanup.is_clean(), "{cleanup:?}");
    assert_eq!(*adapter.cancelled.lock().unwrap(), ["o1".to_owned()]);
}

/// When the relay already shows the order closed while the maker's screen
/// cannot be read, the daemon's word is enough: nobody is asked to cancel.
#[tokio::test]
async fn an_order_the_relay_closed_needs_no_counterpart_when_the_maker_screen_is_unreadable() {
    mortsom_scenarios::link();
    let world = world(true);
    let mut adapter = FakeAdapter::full();
    adapter.fail_release = true;
    adapter.maker_screen_unreadable = true;
    let adapter = Arc::new(adapter);
    let observer = FakeObserver {
        publish_closed: true,
        closed_gate: None,
    };

    let summary = run_with_observer(
        &world,
        adapter.clone(),
        opts_in(&world, &["happy_sell"]),
        true,
        &observer,
    )
    .await
    .unwrap();

    assert!(!summary.passed);
    let cleanup = summary.cleanup.unwrap();
    assert!(cleanup.is_clean(), "{cleanup:?}");
    assert!(
        adapter.cancelled.lock().unwrap().is_empty(),
        "the counterpart was asked to cancel an order the relay had closed"
    );
}

/// An unreadable maker screen never stands in for the daemon: when the
/// relay does not close the order after both sides cancelled, the entry
/// stays unresolved and says why.
#[tokio::test]
async fn an_unreadable_maker_screen_stays_unresolved_until_the_relay_closes_the_order() {
    mortsom_scenarios::link();
    let world = world(true);
    let mut adapter = FakeAdapter::full();
    adapter.fail_release = true;
    adapter.maker_screen_unreadable = true;
    let adapter = Arc::new(adapter);
    let observer = FakeObserver {
        publish_closed: false,
        closed_gate: None,
    };

    let summary = run_with_observer(
        &world,
        adapter.clone(),
        opts_in(&world, &["happy_sell"]),
        true,
        &observer,
    )
    .await
    .unwrap();

    assert!(!summary.passed);
    let cleanup = summary.cleanup.unwrap();
    assert_eq!(cleanup.unresolved, vec!["o1".to_owned()]);
    let entry = &cleanup.entries[0];
    assert!(
        matches!(&entry.result, Some(mortsom_scenario_kit::cleanup::CleanupResult::Failed { message }) if message.contains("relay never showed it closed")),
        "{entry:?}"
    );
    assert_eq!(
        *adapter.cancelled.lock().unwrap(),
        ["o1".to_owned()],
        "the counterpart was still asked to cancel"
    );
}

/// An order the app no longer lists is not thereby closed. The live Web run
/// of 2026-09-08 created an order the client never showed; the daemon kept it
/// pending for an hour. "Not found" on screen needs the relay to show the
/// order closed before cleanup may report it done.
#[tokio::test]
async fn cleanup_needs_the_relay_to_close_an_order_the_app_does_not_list() {
    mortsom_scenarios::link();
    let world = world(true);
    let mut adapter = FakeAdapter::full();
    adapter.fail_release = true;
    adapter.hides_trades = true;
    let adapter = Arc::new(adapter);
    let observer = FakeObserver {
        publish_closed: false,
        closed_gate: None,
    };

    let summary = run_with_observer(
        &world,
        adapter,
        opts_in(&world, &["happy_sell"]),
        true,
        &observer,
    )
    .await
    .unwrap();

    let cleanup = summary.cleanup.unwrap();
    assert_eq!(cleanup.unresolved, vec!["o1".to_owned()]);
    let entry = &cleanup.entries[0];
    assert!(
        matches!(&entry.result, Some(mortsom_scenario_kit::cleanup::CleanupResult::Failed { message }) if message.contains("lists no trade")),
        "{entry:?}"
    );
}

/// The mirror of the previous test: when the relay does show the unlisted
/// order closed, cleanup reports it resolved, so the journal can close.
#[tokio::test]
async fn an_unlisted_order_the_relay_shows_closed_counts_as_resolved() {
    mortsom_scenarios::link();
    let world = world(true);
    let mut adapter = FakeAdapter::full();
    adapter.fail_release = true;
    adapter.hides_trades = true;
    let adapter = Arc::new(adapter);

    let summary = run(&world, adapter, opts_in(&world, &["happy_sell"]), true)
        .await
        .unwrap();

    let cleanup = summary.cleanup.unwrap();
    assert!(cleanup.is_clean(), "{cleanup:?}");
}

/// App v2 drops a cancelled trade that never went active from My Trades,
/// so after the maker's cancel there is no state left to read. The relay
/// says whether the daemon closed it: when it did, the order is resolved.
#[tokio::test]
async fn cleanup_resolves_a_never_active_cancel_the_app_dropped_once_the_relay_closes_it() {
    mortsom_scenarios::link();
    let world = world(false);
    let mut adapter = FakeAdapter::full();
    adapter.fail_take = true;
    adapter.drops_never_active = true;
    let adapter = Arc::new(adapter);

    let summary = run(
        &world,
        adapter.clone(),
        opts_in(&world, &["happy_sell"]),
        false,
    )
    .await
    .unwrap();

    assert!(
        !summary.passed,
        "the take failed; the scenario must not pass"
    );
    let cleanup = summary.cleanup.unwrap();
    assert!(cleanup.is_clean(), "{cleanup:?}");
    assert_eq!(*adapter.cancelled.lock().unwrap(), ["o1".to_owned()]);
}

/// Once the maker's loaded list no longer shows the trade there is no row
/// to open: each try waits out the UI timeout on a device (live, 2026-10-19:
/// `taker_cancel_at_taken` spent 137 s there). The relay decides at once.
#[tokio::test]
async fn cleanup_never_reopens_a_trade_the_maker_app_dropped() {
    mortsom_scenarios::link();
    let world = world(false);
    let mut adapter = FakeAdapter::full();
    adapter.fail_take = true;
    adapter.drops_never_active = true;
    let adapter = Arc::new(adapter);

    let summary = run(
        &world,
        adapter.clone(),
        opts_in(&world, &["happy_sell"]),
        false,
    )
    .await
    .unwrap();

    assert!(summary.cleanup.unwrap().is_clean());
    assert_eq!(*adapter.dropped.lock().unwrap(), ["o1".to_owned()]);
    assert_eq!(*adapter.reopened_dropped.lock().unwrap(), 0);
}

/// The mirror: the app dropping the trade is not the daemon closing it.
/// Without `canceled` on the relay the order stays unresolved.
#[tokio::test]
async fn a_never_active_cancel_the_relay_never_closes_stays_unresolved() {
    mortsom_scenarios::link();
    let world = world(false);
    let mut adapter = FakeAdapter::full();
    adapter.fail_take = true;
    adapter.drops_never_active = true;
    let adapter = Arc::new(adapter);
    let observer = FakeObserver {
        publish_closed: false,
        closed_gate: None,
    };

    let summary = run_with_observer(
        &world,
        adapter,
        opts_in(&world, &["happy_sell"]),
        false,
        &observer,
    )
    .await
    .unwrap();

    let cleanup = summary.cleanup.unwrap();
    assert_eq!(cleanup.unresolved, vec!["o1".to_owned()]);
    let entry = &cleanup.entries[0];
    assert!(
        matches!(&entry.result, Some(mortsom_scenario_kit::cleanup::CleanupResult::Failed { message }) if message.contains("relay never showed it closed")),
        "{entry:?}"
    );
}
