//! What the `run` report must say, and what colour may and may not change.

use super::*;
use mortsom_common::FailureCategory;

const PLAIN: Style = Style::new(false);

fn run(outcome: Option<Outcome>, secs: u64) -> RunView {
    RunView {
        run_id: "20261014T193526Z-e7111efd3bc9".into(),
        run_passed: !matches!(outcome, Some(Outcome::Failed { .. }) | None),
        outcome,
        attempts: 1,
        duration: Duration::from_secs(secs),
        failed_checks: Vec::new(),
        unresolved: Vec::new(),
    }
}

fn failed(category: FailureCategory, message: &str) -> Option<Outcome> {
    Some(Outcome::Failed {
        category,
        message: message.into(),
    })
}

fn skipped(reason: &str) -> Option<Outcome> {
    Some(Outcome::Skipped {
        reason: reason.into(),
    })
}

fn footer() -> Footer<'static> {
    Footer {
        elapsed: Duration::from_secs(760),
        artifacts_dir: Some("artifacts"),
    }
}

#[test]
fn every_scenario_gets_a_badged_line_with_its_duration() {
    // Arrange
    let mut report = Report::default();
    report.record("happy_sell", run(Some(Outcome::Passed), 252));
    report.record(
        "market_price_order",
        run(failed(FailureCategory::Product, "no rate"), 41),
    );
    report.record("range_order", run(skipped("profile lacks range-order"), 0));

    // Act
    let rendered = report.render(&footer(), PLAIN);

    // Assert
    assert!(rendered.contains("[PASS]  happy_sell"), "{rendered}");
    assert!(
        rendered.contains("[FAIL]  market_price_order"),
        "{rendered}"
    );
    assert!(rendered.contains("[SKIP]  range_order"), "{rendered}");
    assert!(rendered.contains("4m 12s"), "{rendered}");
    assert!(rendered.contains("41.0s"), "{rendered}");
}

#[test]
fn a_passing_scenario_takes_exactly_one_line() {
    // Arrange
    let mut report = Report::default();
    report.record("happy_sell", run(Some(Outcome::Passed), 252));

    // Act
    let rendered = report.render(&footer(), PLAIN);

    // Assert: no checks, cleanup or attempt bookkeeping on a pass.
    let lines: Vec<_> = rendered
        .lines()
        .filter(|l| l.contains("happy_sell"))
        .collect();
    assert_eq!(lines.len(), 1, "{rendered}");
    assert!(!rendered.contains("cleanup"), "{rendered}");
    assert!(!rendered.contains("attempt"), "{rendered}");
}

#[test]
fn failure_detail_is_wrapped_under_its_scenario_with_its_category() {
    // Arrange
    let mut report = Report::default();
    let message = "could not return to the order book within 4 back steps: \
        `order.add.fab` did not appear: element `order.add.fab` not found within 30s";
    report.record(
        "cancel_before_taken",
        run(failed(FailureCategory::AutomationContract, message), 95),
    );

    // Act
    let rendered = report.render(&footer(), PLAIN);

    // Assert
    assert!(
        rendered.contains("automation-contract: could not return"),
        "{rendered}"
    );
    assert!(
        rendered
            .lines()
            .all(|l| style::visible_len(l) <= style::REPORT_WIDTH),
        "{rendered}"
    );
}

#[test]
fn a_retried_pass_is_badged_as_flaky() {
    // Arrange
    let mut report = Report::default();
    let mut retried = run(Some(Outcome::Passed), 90);
    retried.attempts = 2;
    report.record("happy_buy", retried);

    // Act
    let rendered = report.render(&footer(), PLAIN);

    // Assert
    assert!(rendered.contains("[FLAKY] happy_buy"), "{rendered}");
    assert!(rendered.contains("passed on attempt 2"), "{rendered}");
    assert!(rendered.contains("passed only on retry"), "{rendered}");
    assert_eq!(report.code(), ExitCode::SUCCESS);
}

#[test]
fn a_passed_scenario_in_a_run_that_did_not_pass_fails_and_says_why() {
    // Arrange
    let mut report = Report::default();
    let mut leaky = run(Some(Outcome::Passed), 120);
    leaky.run_passed = false;
    leaky.unresolved = vec!["f10a9fc6".into()];
    report.record("cancel_after_take_unpaid", leaky);

    // Act
    let rendered = report.render(&footer(), PLAIN);

    // Assert
    assert_eq!(report.code(), ExitCode::from(1));
    assert!(
        rendered.contains("[FAIL]  cancel_after_take_unpaid"),
        "{rendered}"
    );
    assert!(
        rendered.contains("passed, but its run did not"),
        "{rendered}"
    );
    assert!(rendered.contains("unresolved: f10a9fc6"), "{rendered}");
}

#[test]
fn a_preflight_failure_names_the_checks_that_failed() {
    // Arrange
    let mut report = Report::default();
    let mut refused = run(
        failed(
            FailureCategory::EnvironmentMisconfiguration,
            "desktop preflight failed before client startup",
        ),
        0,
    );
    refused.failed_checks = vec!["daemon: no info event within 30s".into()];
    report.record("happy_sell", refused);

    // Act
    let rendered = report.render(&footer(), PLAIN);

    // Assert
    assert!(
        rendered.contains("check daemon: no info event within 30s"),
        "{rendered}"
    );
}

#[test]
fn a_run_without_a_result_for_its_scenario_counts_as_failed() {
    // Arrange
    let mut report = Report::default();
    report.record("happy_sell", run(None, 0));

    // Act
    let rendered = report.render(&footer(), PLAIN);

    // Assert
    assert_eq!(report.code(), ExitCode::from(1));
    assert!(rendered.contains("produced no result for it"), "{rendered}");
}

#[test]
fn a_refused_run_stops_the_family_and_lists_the_rest_as_not_run() {
    // Arrange
    let mut report = Report::default();
    report.record("happy_sell", run(Some(Outcome::Passed), 252));
    report.refuse("happy_buy", "no ready stack: run `mortsom stack up`");
    report.not_run(["range_order", "market_price_order"].into_iter());

    // Act
    let rendered = report.render(&footer(), PLAIN);

    // Assert
    assert_eq!(report.code(), ExitCode::from(2));
    assert_eq!(report.finished(), 4);
    assert!(rendered.contains("[STOP]  happy_buy"), "{rendered}");
    assert!(rendered.contains("no ready stack"), "{rendered}");
    assert!(rendered.contains("[STOP]  range_order"), "{rendered}");
    assert!(rendered.contains("3 scenarios not run"), "{rendered}");
    assert!(
        rendered.contains("not run: happy_buy, range_order, market_price_order"),
        "{rendered}"
    );
    assert!(!rendered.contains("all scenarios passed"), "{rendered}");
}

#[test]
fn a_refusal_before_any_run_points_at_no_artifacts() {
    // Arrange
    let mut report = Report::default();
    report.refuse("happy_sell", "no ready stack");
    let footer = Footer {
        elapsed: Duration::from_millis(30),
        artifacts_dir: None,
    };

    // Act
    let rendered = report.render(&footer, PLAIN);

    // Assert
    assert!(!rendered.contains("artifacts"), "{rendered}");
    assert!(rendered.contains("1 scenario not run"), "{rendered}");
}

#[test]
fn footer_carries_counts_total_time_and_artifacts() {
    // Arrange
    let mut report = Report::default();
    report.record("happy_sell", run(Some(Outcome::Passed), 252));
    report.record(
        "market_price_order",
        run(failed(FailureCategory::Product, "no rate"), 41),
    );

    // Act
    let rendered = report.render(&footer(), PLAIN);

    // Assert
    assert!(
        rendered.contains("1 passed  1 failed  0 skipped  |  12m 40s"),
        "{rendered}"
    );
    assert!(rendered.contains("artifacts artifacts\n"), "{rendered}");
    assert!(rendered.contains("1 scenario failed"), "{rendered}");
    assert!(
        rendered.contains("failed: market_price_order"),
        "{rendered}"
    );
}

#[test]
fn a_failed_scenario_among_several_names_its_run() {
    // Arrange
    let mut report = Report::default();
    report.record("happy_sell", run(Some(Outcome::Passed), 252));
    report.record(
        "market_price_order",
        run(failed(FailureCategory::Product, "no rate"), 41),
    );

    // Act
    let rendered = report.render(&footer(), PLAIN);

    // Assert: the failed run is the one an operator cleans up or reads.
    assert!(
        rendered.contains("run 20261014T193526Z-e7111efd3bc9"),
        "{rendered}"
    );
    assert_eq!(
        rendered.matches("20261014T193526Z").count(),
        1,
        "{rendered}"
    );
}

#[test]
fn a_single_run_points_at_its_own_artifacts_directory() {
    // Arrange
    let mut report = Report::default();
    report.record("happy_sell", run(Some(Outcome::Passed), 252));

    // Act
    let rendered = report.render(&footer(), PLAIN);

    // Assert
    assert!(
        rendered.contains("artifacts artifacts/20261014T193526Z-e7111efd3bc9"),
        "{rendered}"
    );
}

#[test]
fn a_clean_run_says_so() {
    // Arrange
    let mut report = Report::default();
    report.record("happy_sell", run(Some(Outcome::Passed), 252));

    // Act
    let rendered = report.render(&footer(), PLAIN);

    // Assert
    assert_eq!(report.code(), ExitCode::SUCCESS);
    assert!(rendered.contains("✓ all scenarios passed"), "{rendered}");
    assert!(!rendered.contains("failed:"), "{rendered}");
}

#[test]
fn skipped_scenarios_qualify_the_verdict_without_failing_it() {
    // Arrange
    let mut report = Report::default();
    report.record("happy_sell", run(Some(Outcome::Passed), 252));
    report.record("range_order", run(skipped("profile lacks range-order"), 0));

    // Act
    let rendered = report.render(&footer(), PLAIN);

    // Assert
    assert_eq!(report.code(), ExitCode::SUCCESS);
    assert!(rendered.contains("profile lacks range-order"), "{rendered}");
    assert!(rendered.contains("✓ no failures (1 skipped)"), "{rendered}");
}

#[test]
fn a_run_where_nothing_ran_is_not_a_pass() {
    // Arrange
    let mut report = Report::default();
    report.record("background_mid_trade", run(skipped("needs background"), 0));

    // Act
    let rendered = report.render(&footer(), PLAIN);

    // Assert
    assert!(!rendered.contains("all scenarios passed"), "{rendered}");
    assert!(rendered.contains("nothing ran: 1 skipped"), "{rendered}");
}

#[test]
fn plain_output_carries_no_escape_sequences_and_colour_does() {
    // Arrange
    let mut report = Report::default();
    report.record("happy_sell", run(Some(Outcome::Passed), 252));

    // Act
    let plain = report.render(&footer(), PLAIN);
    let coloured = report.render(&footer(), Style::new(true));

    // Assert
    assert!(!plain.contains('\u{1b}'), "{plain}");
    assert!(coloured.contains('\u{1b}'), "{coloured}");
}

#[test]
fn every_status_has_its_own_colour() {
    // Arrange
    let mut palette = [
        Status::Passed,
        Status::Flaky,
        Status::Failed,
        Status::Skipped,
        Status::NotRun,
    ]
    .map(Status::color)
    .to_vec();

    // Act
    palette.sort_unstable();
    palette.dedup();

    // Assert
    assert_eq!(palette.len(), 5, "two statuses share a colour");
}

#[test]
fn the_header_says_how_many_runs_start_and_how_many_move_sats() {
    // Arrange / Act
    let many = header(12, 9, PLAIN);
    let one = header(1, 1, PLAIN);

    // Assert (spec §10: before the first run).
    assert!(many.contains("12 scenarios · 9 move sats"), "{many}");
    assert!(many.contains("its own spend cap"), "{many}");
    assert!(one.contains("1 scenario"), "{one}");
    assert!(!one.contains("spend cap"), "{one}");
}

#[test]
fn progress_counts_the_scenario_it_starts() {
    // Arrange / Act
    let line = progress(2, 12, "happy_buy", PLAIN);

    // Assert
    assert_eq!(line, "  [2/12] happy_buy");
}

#[test]
fn durations_read_as_an_operator_reads_them() {
    // Arrange / Act / Assert
    assert_eq!(human_duration(Duration::ZERO), "-");
    assert_eq!(human_duration(Duration::from_millis(8_040)), "8.0s");
    assert_eq!(human_duration(Duration::from_secs(252)), "4m 12s");
    assert_eq!(human_duration(Duration::from_secs(3725)), "1h 02m 05s");
}
