#![allow(clippy::unwrap_used)]
//! Which failures the stack is blamed for, decided from the probes taken
//! once the scenario has failed, and when the daemon counts as heard.
use std::time::Duration;

use mortsom_common::FailureCategory;
use mortsom_common::check::{CheckStatus, DoctorReport};
use mortsom_environment::daemon::{DaemonInfo, DaemonProbe, RelayProbe};
use mortsom_scenario_kit::outcome::Outcome;

use super::{attribute, daemon_heard};

const NOW: u64 = 1_792_069_200;
const EVERY_30S: Result<Duration, String> = Ok(Duration::from_secs(30));

/// A daemon probe through one relay, which answered unless `relay_error`,
/// with an info event `info_age` seconds old.
fn probe(relay_error: Option<&str>, info_age: Option<u64>) -> DaemonProbe {
    DaemonProbe {
        relays: vec![RelayProbe {
            configured: "ws://relay:7500".into(),
            dialled: "ws://127.0.0.1:7500".into(),
            error: relay_error.map(str::to_owned),
        }],
        info: info_age.map(|age| DaemonInfo {
            pubkey: "c94a16a5".into(),
            created_at: NOW - age,
            tags: Default::default(),
        }),
        relay_list: None,
    }
}

#[test]
fn a_daemon_silent_behind_a_live_relay_is_not_heard() {
    // Act
    let (status, detail) = daemon_heard(&probe(None, Some(600)), EVERY_30S, NOW);

    // Assert
    assert_eq!(status, CheckStatus::Fail);
    assert!(detail.contains("600s old"), "{detail}");
    assert!(detail.contains("every 30s"), "{detail}");
}

#[test]
fn a_daemon_publishing_on_its_interval_is_heard() {
    // Act
    let (status, detail) = daemon_heard(&probe(None, Some(25)), EVERY_30S, NOW);

    // Assert
    assert_eq!(status, CheckStatus::Pass, "{detail}");
}

#[test]
fn a_daemon_with_no_info_event_on_a_live_relay_is_not_heard() {
    // Act
    let (status, _) = daemon_heard(&probe(None, None), EVERY_30S, NOW);

    // Assert
    assert_eq!(status, CheckStatus::Fail);
}

#[test]
fn the_daemon_is_not_judged_without_its_interval_or_a_relay_to_hear_it_on() {
    for (probe, interval) in [
        (
            probe(None, Some(600)),
            Err("no publish_mostro_info_interval".to_owned()),
        ),
        (probe(Some("connection refused"), None), EVERY_30S),
    ] {
        // Act
        let (status, detail) = daemon_heard(&probe, interval, NOW);

        // Assert
        assert_eq!(status, CheckStatus::Skip, "{detail}");
    }
}

fn failed(category: FailureCategory, message: &str) -> Outcome {
    Outcome::Failed {
        category,
        message: message.to_owned(),
    }
}

fn healthy() -> DoctorReport {
    let mut report = DoctorReport::default();
    report.push("relay-port", CheckStatus::Pass, "127.0.0.1:7500 listening");
    report.push("mostro-daemon", CheckStatus::Pass, "info event found");
    report
}

fn stack_down() -> DoctorReport {
    let mut report = DoctorReport::default();
    report.push(
        "relay-port",
        CheckStatus::Fail,
        "nothing listens on 127.0.0.1:7500",
    );
    report.push("mostro-daemon", CheckStatus::Fail, "no info event");
    report.push("wallet alice", CheckStatus::Pass, "regtest");
    report
}

#[test]
fn a_failure_while_the_stack_is_down_is_blamed_on_the_infrastructure() {
    // Arrange
    let outcome = failed(
        FailureCategory::Product,
        "no new trade appeared in the trades list after creating the order",
    );

    // Act
    let attributed = attribute(outcome, &stack_down());

    // Assert
    let Outcome::Failed { category, message } = attributed else {
        panic!("still a failure");
    };
    assert_eq!(category, FailureCategory::TestInfrastructure);
    assert!(
        message.contains("relay-port: nothing listens on 127.0.0.1:7500"),
        "{message}"
    );
    assert!(
        message.contains("mostro-daemon: no info event"),
        "{message}"
    );
    assert!(!message.contains("wallet alice"), "{message}");
    assert!(
        message
            .contains("product: no new trade appeared in the trades list after creating the order"),
        "{message}"
    );
}

#[test]
fn a_failure_with_the_stack_still_ready_keeps_its_own_category() {
    // Arrange
    let outcome = failed(
        FailureCategory::AutomationContract,
        "`order.status` did not appear",
    );

    // Act
    let attributed = attribute(outcome.clone(), &healthy());

    // Assert
    assert_eq!(attributed, outcome);
}

#[test]
fn an_infrastructure_failure_is_left_as_reported() {
    // Arrange
    let outcome = failed(FailureCategory::TestInfrastructure, "session lost");

    // Act
    let attributed = attribute(outcome.clone(), &stack_down());

    // Assert
    assert_eq!(attributed, outcome);
}

#[test]
fn a_pass_or_a_skip_is_never_rewritten() {
    for outcome in [
        Outcome::Passed,
        Outcome::Skipped {
            reason: "needs relay-faults".into(),
        },
    ] {
        assert_eq!(attribute(outcome.clone(), &stack_down()), outcome);
    }
}
