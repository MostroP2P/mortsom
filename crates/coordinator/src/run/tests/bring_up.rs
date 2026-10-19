//! Bring-up bootstraps both actors at once and reports them in actor order.
use super::*;
use tokio::sync::Barrier;

/// Each actor's bootstrap checks, in the order the run reports them.
fn actor_checks(summary: &RunSummary) -> Vec<(String, CheckStatus)> {
    summary
        .checks
        .checks
        .iter()
        .filter(|check| check.name.starts_with("alice ") || check.name.starts_with("bob "))
        .map(|check| (check.name.clone(), check.status))
        .collect()
}

/// Where `phase` of `actor` ended among the run's phases.
fn ended_at(timings: &serde_json::Value, phase: &str, actor: &str) -> usize {
    timings["phases"]
        .as_array()
        .unwrap()
        .iter()
        .position(|entry| entry["phase"] == phase && entry["actor"] == actor)
        .unwrap_or_else(|| panic!("{phase} of {actor} missing from {timings}"))
}

/// One actor's bootstrap does not wait for the other's: each waits at
/// onboarding until the other got there too.
#[tokio::test]
async fn alice_and_bob_bootstrap_at_the_same_time() {
    mortsom_scenarios::link();
    let world = world(true);
    let adapter = Arc::new(FakeAdapter {
        onboarding_meets: Some(Arc::new(Barrier::new(2))),
        ..FakeAdapter::full()
    });

    let summary = tokio::time::timeout(
        Duration::from_secs(10),
        run(&world, adapter, opts_in(&world, &["happy_sell"]), true),
    )
    .await
    .expect("one actor's bootstrap waited for the other's to end")
    .unwrap();

    assert!(summary.passed, "{}", summary.checks.render());
}

/// The report reads as it always has, Alice's steps and then Bob's, even
/// when Bob is ready first.
#[tokio::test]
async fn the_bootstrap_checks_keep_alice_before_bob_whoever_finishes_first() {
    mortsom_scenarios::link();
    let world = world(true);
    let adapter = Arc::new(FakeAdapter {
        slow_onboarding: Some((Actor::Alice, Duration::from_millis(300))),
        ..FakeAdapter::full()
    });

    let summary = run(&world, adapter, opts_in(&world, &["happy_sell"]), true)
        .await
        .unwrap();

    let timings = timings_of(&world, &summary);
    assert!(
        ended_at(&timings, "bootstrap/restart", "bob")
            < ended_at(&timings, "bootstrap/restart", "alice"),
        "Bob was meant to be ready first: {timings}"
    );
    let mut expected = Vec::new();
    for actor in ["alice", "bob"] {
        for (step, status) in [
            ("launch", CheckStatus::Pass),
            ("onboarding", CheckStatus::Pass),
            ("node-and-relay", CheckStatus::Pass),
            ("nwc", CheckStatus::Skip),
            ("restart", CheckStatus::Pass),
            ("identity-survives-restart", CheckStatus::Pass),
        ] {
            expected.push((format!("{actor} {step}"), status));
        }
    }
    assert_eq!(actor_checks(&summary), expected);
    let actors: Vec<_> = summary.actors.iter().map(|a| a.actor.as_str()).collect();
    assert_eq!(actors, ["alice", "bob"]);
    assert!(summary.passed, "{}", summary.checks.render());
}

/// A failed step still ends that actor's journey and fails the bring-up;
/// the other actor's journey is reported in full.
#[tokio::test]
async fn a_failed_bootstrap_still_fails_the_bring_up() {
    mortsom_scenarios::link();
    let world = world(true);
    let adapter = Arc::new(FakeAdapter {
        failed_onboarding: Some(Actor::Bob),
        ..FakeAdapter::full()
    });

    let summary = run(&world, adapter, opts_in(&world, &["happy_sell"]), true)
        .await
        .unwrap();

    assert!(!summary.passed);
    let checks = actor_checks(&summary);
    let status = |name: &str| {
        checks
            .iter()
            .find(|(check, _)| check == name)
            .map(|(_, status)| *status)
    };
    assert_eq!(
        status("alice identity-survives-restart"),
        Some(CheckStatus::Pass)
    );
    assert_eq!(status("bob onboarding"), Some(CheckStatus::Fail));
    assert_eq!(status("bob node-and-relay"), None);
    assert!(
        matches!(
            &summary.results[0].outcome,
            Outcome::Failed { message, .. } if message.contains("environment did not come up")
        ),
        "{:?}",
        summary.results
    );
}
