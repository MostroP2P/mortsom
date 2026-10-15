#![allow(clippy::unwrap_used)]
//! Which failures the stack is blamed for, decided from the readiness
//! checks taken once the scenario has failed.
use mortsom_common::FailureCategory;
use mortsom_common::check::{CheckStatus, DoctorReport};
use mortsom_scenario_kit::outcome::Outcome;

use super::attribute;

fn failed(category: FailureCategory, message: &str) -> Outcome {
    Outcome::Failed {
        category,
        message: message.to_owned(),
    }
}

fn healthy() -> DoctorReport {
    let mut report = DoctorReport::default();
    report.push("relay-port", CheckStatus::Pass, "127.0.0.1:7000 listening");
    report.push("mostro-daemon", CheckStatus::Pass, "info event found");
    report
}

fn stack_down() -> DoctorReport {
    let mut report = DoctorReport::default();
    report.push(
        "relay-port",
        CheckStatus::Fail,
        "nothing listens on 127.0.0.1:7000",
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
        message.contains("relay-port: nothing listens on 127.0.0.1:7000"),
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
