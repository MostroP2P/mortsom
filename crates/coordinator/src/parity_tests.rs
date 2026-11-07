#![allow(clippy::unwrap_used, clippy::expect_used)]
use super::*;

fn summary(run_id: &str, profile: &str, outcomes: &[(&str, &str)]) -> String {
    let results: Vec<String> = outcomes
        .iter()
        .map(|(name, outcome)| {
            let o = match *outcome {
                "passed" => r#"{"outcome": "passed"}"#.to_owned(),
                "skipped" => r#"{"outcome": "skipped", "reason": "not offered"}"#.to_owned(),
                "contract" => r#"{"outcome": "failed", "category": "automation-contract", "message": "id missing"}"#.to_owned(),
                other => format!(r#"{{"outcome": "failed", "category": "{other}", "message": "x"}}"#),
            };
            format!(
                r#"{{"name": "{name}", "app_profile": "{profile}", "app_version": "1", "outcome": {o}, "attempts": 1, "duration": {{"secs": 1, "nanos": 0}}}}"#
            )
        })
        .collect();
    format!(
        r#"{{"run_id": "{run_id}", "lane": "ci", "command": "run", "app_profile": "{profile}", "app_version": "1",
  "mainnet_cleared": true, "relay": null, "actors": [], "results": [{}], "checks": {{"checks": []}},
  "cleanup": {{"entries": [], "unresolved": ["o2"]}}, "budget": null, "farm_teardown": [], "relay_teardown": null, "passed": true}}"#,
        results.join(",")
    )
}

fn entry(kind: TimelineKind, actor: Option<&str>, summary: &str) -> TimelineEntry {
    TimelineEntry {
        at_ms: 1,
        kind,
        actor: actor.map(str::to_owned),
        summary: summary.into(),
        artifact: None,
    }
}

fn timeline(states: &[(&str, &str)], relay: &[&str], wallet: bool) -> String {
    let mut entries = vec![entry(
        TimelineKind::Step,
        None,
        "scenario happy_sell attempt 1 started",
    )];
    entries.push(entry(
        TimelineKind::Step,
        Some("alice"),
        "created Sell order o1",
    ));
    for (actor, state) in states {
        entries.push(entry(
            TimelineKind::UiState,
            Some(actor),
            &format!("order o1 shows {state} (`raw|label`)"),
        ));
    }
    for state in relay {
        entries.push(entry(
            TimelineKind::Assertion,
            None,
            &format!("relay: order o1 is {state}"),
        ));
    }
    if wallet {
        entries.push(entry(
            TimelineKind::Wallet,
            Some("bob"),
            "wallet: 1000 sats settled for order o1",
        ));
    }
    entries.push(entry(
        TimelineKind::Step,
        None,
        "scenario other attempt 1 started",
    ));
    entries.push(entry(
        TimelineKind::UiState,
        Some("alice"),
        "order o2 shows cancelled (`x`)",
    ));
    serde_json::to_string(&entries).unwrap()
}

fn write_run(dir: &Path, id: &str, summary_json: &str, timeline_json: &str) {
    let run = dir.join(id);
    std::fs::create_dir_all(&run).unwrap();
    std::fs::write(run.join(files::SUMMARY), summary_json).unwrap();
    if !timeline_json.is_empty() {
        std::fs::write(run.join(files::TIMELINE_JSON), timeline_json).unwrap();
    }
}

#[test]
fn an_abandoned_recovery_never_reaches_parity() {
    let dir = tempfile::tempdir().unwrap();
    let states = [("alice", "pending")];
    for (id, profile) in [("a", "profile-b"), ("b", "mobile-v2")] {
        write_run(
            dir.path(),
            id,
            &summary(id, profile, &[("happy_sell", "passed")]),
            &timeline(&states, &["pending"], false),
        );
    }
    assert!(compare_runs(dir.path(), "a", "b", &[]).unwrap().parity);

    crate::recovery_abandon::write_test_artifact(&dir.path().join("b"));
    let accepted = [Intentional {
        scenario: "happy_sell".into(),
        aspect: Aspect::Cleanup,
        reason: "accepting it changes nothing".into(),
        profiles: Vec::new(),
    }];
    let report = compare_runs(dir.path(), "a", "b", &accepted).unwrap();
    assert!(!report.parity);
    assert_eq!(report.abandoned_runs, vec!["b".to_owned()]);
    assert!(report.scenarios[0].b.recovery_abandoned);
    assert!(report.render().contains("ABANDONED recovery"));
    let findings = compare_runs(dir.path(), "a", "b", &[]).unwrap();
    assert_eq!(findings.scenarios[0].findings, vec![Aspect::Cleanup]);
}

#[test]
fn the_result_line_names_abandonment_when_it_is_the_only_reason() {
    let dir = tempfile::tempdir().unwrap();
    let states = [("alice", "pending")];
    for (id, profile) in [("a", "profile-b"), ("b", "mobile-v2")] {
        write_run(
            dir.path(),
            id,
            &summary(id, profile, &[("happy_sell", "passed")]),
            &timeline(&states, &["pending"], false),
        );
    }
    crate::recovery_abandon::write_test_artifact(&dir.path().join("b"));
    let accepted = [Intentional {
        scenario: "happy_sell".into(),
        aspect: Aspect::Cleanup,
        reason: "accepted".into(),
        profiles: Vec::new(),
    }];

    let only_abandonment = compare_runs(dir.path(), "a", "b", &accepted)
        .unwrap()
        .render();
    let with_findings = compare_runs(dir.path(), "a", "b", &[]).unwrap().render();

    assert!(only_abandonment.contains("result: FINDINGS (a run's recovery was abandoned"));
    assert!(!only_abandonment.contains("unexplained differences"));
    assert!(with_findings.contains("result: FINDINGS (unexplained differences above)"));
}

#[test]
fn a_prepared_or_unreadable_abandonment_record_never_reaches_parity() {
    for corrupt in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let states = [("alice", "pending")];
        for (id, profile) in [("a", "profile-b"), ("b", "mobile-v2")] {
            write_run(
                dir.path(),
                id,
                &summary(id, profile, &[("happy_sell", "passed")]),
                &timeline(&states, &["pending"], false),
            );
        }
        crate::recovery_abandon::write_incomplete_test_artifacts(&dir.path().join("b"), corrupt);

        let report = compare_runs(dir.path(), "a", "b", &[]).unwrap();

        assert!(!report.parity, "corrupt={corrupt}");
        assert_eq!(report.abandoned_runs, vec!["b".to_owned()]);
    }
}

#[test]
fn identical_journeys_reach_parity_and_differences_become_findings_or_intentional() {
    let dir = tempfile::tempdir().unwrap();
    let states = [
        ("alice", "pending"),
        ("alice", "taken"),
        ("bob", "taken"),
        ("alice", "active"),
        ("bob", "completed"),
    ];
    write_run(
        dir.path(),
        "a",
        &summary(
            "a",
            "profile-b",
            &[
                ("happy_sell", "passed"),
                ("other", "passed"),
                ("legacy_only", "passed"),
            ],
        ),
        &timeline(&states, &["pending", "active", "success"], true),
    );
    write_run(
        dir.path(),
        "b",
        &summary(
            "b",
            "mobile-v2",
            &[("happy_sell", "passed"), ("other", "contract")],
        ),
        &timeline(&states, &["pending", "active", "success"], true),
    );

    let report = compare_runs(dir.path(), "a", "b", &[]).unwrap();
    assert_eq!(report.only_in_a, vec!["legacy_only".to_owned()]);
    let happy = report
        .scenarios
        .iter()
        .find(|s| s.name == "happy_sell")
        .unwrap();
    assert!(happy.findings.is_empty(), "{happy:?}");
    assert_eq!(
        happy.a.unresolved_orders, 0,
        "o2 belongs to another scenario"
    );
    assert_eq!(
        report
            .scenarios
            .iter()
            .find(|s| s.name == "other")
            .unwrap()
            .a
            .unresolved_orders,
        1
    );
    assert_eq!(
        happy.a.state_path["alice"],
        vec!["pending", "taken", "active"]
    );
    assert_eq!(happy.a.relay_path, vec!["pending", "active", "success"]);
    assert!(happy.a.wallet_settled && happy.b.wallet_settled);
    let other = report.scenarios.iter().find(|s| s.name == "other").unwrap();
    assert!(other.findings.contains(&Aspect::Outcome));
    assert!(other.findings.contains(&Aspect::Contract));
    assert!(!report.parity);
    assert!(dir.path().join(PARITY_MD).is_file());

    let intentional = vec![
        Intentional {
            scenario: "other".into(),
            aspect: Aspect::Outcome,
            reason: "v2 lacks the id on purpose".into(),
            profiles: Vec::new(),
        },
        Intentional {
            scenario: "other".into(),
            aspect: Aspect::Contract,
            reason: "same".into(),
            profiles: Vec::new(),
        },
    ];
    let report = compare_runs(dir.path(), "a", "b", &intentional).unwrap();
    assert!(report.parity, "{}", report.render());
    assert!(report.render().contains("v2 lacks the id on purpose"));
}

#[test]
fn same_profile_and_missing_runs_are_errors() {
    let dir = tempfile::tempdir().unwrap();
    write_run(dir.path(), "a", &summary("a", "profile-b", &[]), "");
    write_run(dir.path(), "b", &summary("b", "profile-b", &[]), "");
    assert!(
        compare_runs(dir.path(), "a", "b", &[])
            .unwrap_err()
            .contains("both runs use profile")
    );
    assert!(compare_runs(dir.path(), "a", "nope", &[]).is_err());
}

#[test]
fn two_runs_that_share_no_scenario_are_refused_instead_of_reaching_parity() {
    // Arrange: one scenario's run under each profile, but not the same one.
    let dir = tempfile::tempdir().unwrap();
    let states = [("alice", "pending")];
    for (id, profile, scenario) in [("a", "mobile-v2", "happy_sell"), ("b", "cli", "happy_buy")] {
        write_run(
            dir.path(),
            id,
            &summary(id, profile, &[(scenario, "passed")]),
            &timeline(&states, &["pending"], false),
        );
    }

    // Act
    let err = compare_runs(dir.path(), "a", "b", &[]).unwrap_err();

    // Assert: refused with both runs' scenarios, and no report claims parity.
    assert!(err.contains("share no scenario"), "{err}");
    assert!(err.contains("(a: happy_sell; b: happy_buy)"), "{err}");
    assert!(!dir.path().join(PARITY_JSON).exists());
    assert!(!dir.path().join(PARITY_MD).exists());
}

#[test]
fn a_run_that_carries_no_scenario_never_reaches_parity() {
    // Arrange: a run that ended before its scenario recorded a result.
    let dir = tempfile::tempdir().unwrap();
    write_run(dir.path(), "a", &summary("a", "mobile-v2", &[]), "");
    write_run(
        dir.path(),
        "b",
        &summary("b", "cli", &[("happy_sell", "passed")]),
        "",
    );

    // Act
    let err = compare_runs(dir.path(), "a", "b", &[]).unwrap_err();

    // Assert
    assert!(err.contains("share no scenario"), "{err}");
    assert!(err.contains("(a: none; b: happy_sell)"), "{err}");
    assert!(!dir.path().join(PARITY_JSON).exists());
    assert!(!dir.path().join(PARITY_MD).exists());
}

#[test]
fn a_scenario_only_one_run_carries_is_listed_but_never_compared() {
    // Arrange: the failed scenario only run `a` carries must not count.
    let dir = tempfile::tempdir().unwrap();
    let states = [("alice", "pending")];
    let results_a = [("happy_sell", "passed"), ("legacy_only", "product")];
    let results_b = [("happy_sell", "passed")];
    for (id, profile, results) in [
        ("a", "mobile-v2", &results_a[..]),
        ("b", "cli", &results_b[..]),
    ] {
        write_run(
            dir.path(),
            id,
            &summary(id, profile, results),
            &timeline(&states, &["pending"], false),
        );
    }

    // Act
    let report = compare_runs(dir.path(), "a", "b", &[]).unwrap();

    // Assert
    let compared: Vec<&str> = report.scenarios.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(compared, ["happy_sell"]);
    assert_eq!(report.only_in_a, ["legacy_only"]);
    assert!(report.only_in_b.is_empty());
    assert!(report.parity, "{}", report.render());
}

/// Writes runs `a` (mobile-v2) and `b` (cli) with the same journey and
/// the given results.
fn write_pair(dir: &Path, results_a: &[(&str, &str)], results_b: &[(&str, &str)]) {
    let states = [("alice", "pending")];
    for (id, profile, results) in [("a", "mobile-v2", results_a), ("b", "cli", results_b)] {
        write_run(
            dir,
            id,
            &summary(id, profile, results),
            &timeline(&states, &["pending"], false),
        );
    }
}

#[test]
fn two_runs_that_both_skipped_their_only_shared_scenario_are_refused() {
    // Arrange: each profile skipped the scenario, so neither ran it.
    let dir = tempfile::tempdir().unwrap();
    let skipped = [("happy_sell", "skipped")];
    write_pair(dir.path(), &skipped, &skipped);

    // Act
    let err = compare_runs(dir.path(), "a", "b", &[]).unwrap_err();

    // Assert: refused like two runs that share no scenario, and no report.
    assert!(err.contains("all skipped in both runs"), "{err}");
    assert!(err.contains("(happy_sell)"), "{err}");
    assert!(!dir.path().join(PARITY_JSON).exists());
    assert!(!dir.path().join(PARITY_MD).exists());
}

#[test]
fn a_scenario_both_runs_skipped_is_left_out_and_the_rest_is_compared() {
    // Arrange
    let dir = tempfile::tempdir().unwrap();
    let results = [("happy_sell", "passed"), ("other", "skipped")];
    write_pair(dir.path(), &results, &results);

    // Act
    let report = compare_runs(dir.path(), "a", "b", &[]).unwrap();

    // Assert: only the scenario a run executed is compared.
    let compared: Vec<&str> = report.scenarios.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(compared, ["happy_sell"]);
    assert!(report.only_in_a.is_empty() && report.only_in_b.is_empty());
    assert!(report.parity, "{}", report.render());
    assert!(!report.render().contains("| other |"));
}

#[test]
fn a_scenario_skipped_on_one_side_only_is_still_a_finding() {
    // Arrange
    let dir = tempfile::tempdir().unwrap();
    write_pair(
        dir.path(),
        &[("happy_sell", "skipped")],
        &[("happy_sell", "passed")],
    );

    // Act
    let report = compare_runs(dir.path(), "a", "b", &[]).unwrap();

    // Assert
    assert_eq!(report.scenarios.len(), 1);
    assert_eq!(report.scenarios[0].a.outcome, "skipped");
    assert_eq!(report.scenarios[0].findings, vec![Aspect::Outcome]);
    assert!(!report.parity);
}

#[test]
fn missing_timeline_is_empty_but_unreadable_timeline_is_an_error() {
    let dir = tempfile::tempdir().unwrap();
    let results = [("happy_sell", "passed")];
    write_run(dir.path(), "a", &summary("a", "profile-b", &results), "");
    write_run(dir.path(), "b", &summary("b", "mobile-v2", &results), "");
    assert!(compare_runs(dir.path(), "a", "b", &[]).is_ok());

    // A timeline path that exists but cannot be read as a file must not
    // silently compare as an empty timeline.
    std::fs::create_dir(dir.path().join("b").join(files::TIMELINE_JSON)).unwrap();
    let err = compare_runs(dir.path(), "a", "b", &[]).unwrap_err();
    assert!(err.contains(files::TIMELINE_JSON), "{err}");
}

#[test]
fn intentional_file_parses() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("parity.toml");
    std::fs::write(
        &path,
        "[[intentional]]\nscenario = \"happy_sell\"\naspect = \"state-path\"\nreason = \"v2 shows pending-rating after success\"\n",
    )
    .unwrap();
    let list = load_intentional(&path).unwrap();
    assert_eq!(list[0].aspect, Aspect::StatePath);
    assert!(list[0].profiles.is_empty());
}

#[test]
fn an_entry_with_a_misspelt_key_is_refused_instead_of_accepting_every_pair() {
    // Arrange: `profile` for `profiles` used to load as an unscoped entry.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("parity.toml");
    std::fs::write(
        &path,
        "[[intentional]]\nscenario = \"happy_sell\"\naspect = \"state-path\"\nreason = \"r\"\nprofile = [\"mobile-v2\", \"cli\"]\n",
    )
    .unwrap();

    // Act
    let err = load_intentional(&path).unwrap_err();

    // Assert
    assert!(err.contains("unknown field `profile`"), "{err}");
    assert!(err.contains("parity.toml"), "{err}");
}

#[test]
fn a_misspelt_table_name_is_refused_instead_of_loading_no_entry() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("parity.toml");
    std::fs::write(
        &path,
        "[[intentionals]]\nscenario = \"happy_sell\"\naspect = \"state-path\"\nreason = \"r\"\n",
    )
    .unwrap();

    let err = load_intentional(&path).unwrap_err();

    assert!(err.contains("unknown field `intentionals`"), "{err}");
}

#[test]
fn the_shipped_template_loads_and_its_commented_sample_parses() {
    // Arrange
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../config/parity-intentional.toml");
    let text = std::fs::read_to_string(&path).unwrap();
    let (_, sample) = text.split_once("\n\n").unwrap();
    let uncommented: Vec<&str> = sample
        .lines()
        .map(|line| line.strip_prefix("# ").unwrap_or(line))
        .collect();

    // Act
    let shipped = load_intentional(&path).unwrap();
    let sample: IntentionalFile = toml::from_str(&uncommented.join("\n")).unwrap();

    // Assert: every key the sample documents is one the loader accepts.
    assert!(shipped.is_empty());
    assert_eq!(sample.intentional.len(), 1);
    assert_eq!(sample.intentional[0].profiles, ["mobile-v2", "cli"]);
}

#[test]
fn an_entry_scoped_to_profiles_applies_only_between_them() {
    let entry: Intentional = toml::from_str(
        "scenario = \"cancel_before_taken\"\naspect = \"state-path\"\nreason = \"the CLI reads the daemon's canceled\"\nprofiles = [\"mobile-v2\", \"cli\"]\n",
    )
    .unwrap();
    let open = Intentional {
        profiles: Vec::new(),
        ..entry.clone()
    };

    assert!(entry.accepts(
        "cancel_before_taken",
        Aspect::StatePath,
        ("mobile-v2", "cli")
    ));
    assert!(entry.accepts(
        "cancel_before_taken",
        Aspect::StatePath,
        ("cli", "mobile-v2")
    ));
    assert!(!entry.accepts(
        "cancel_before_taken",
        Aspect::StatePath,
        ("mobile-v2", "mobile-v3")
    ));
    assert!(!entry.accepts("cancel_before_taken", Aspect::Outcome, ("mobile-v2", "cli")));
    assert!(open.accepts(
        "cancel_before_taken",
        Aspect::StatePath,
        ("mobile-v2", "mobile-v3")
    ));
}
