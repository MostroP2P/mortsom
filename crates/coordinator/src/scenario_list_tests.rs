//! What `mortsom list` must say, and what colour may and may not change.

use super::*;

fn listing() -> Listing {
    Listing {
        baseline: vec!["onboarding".to_owned(), "create-order".to_owned()],
        rows: vec![
            Row {
                name: "happy_sell".to_owned(),
                family: "happy-path".to_owned(),
                labels: vec!["smoke".to_owned(), "parity".to_owned()],
                extras: vec!["release".to_owned()],
                timeout_secs: 1800,
                moves_value: true,
                needs_solver: false,
                needs_app_wallet: false,
                needs: Vec::new(),
            },
            Row {
                name: "dispute_settled_by_solver".to_owned(),
                family: "dispute".to_owned(),
                labels: Vec::new(),
                extras: vec!["dispute".to_owned()],
                timeout_secs: 2100,
                moves_value: true,
                needs_solver: true,
                needs_app_wallet: false,
                needs: Vec::new(),
            },
            Row {
                name: "restart_at_pending".to_owned(),
                family: "recovery".to_owned(),
                labels: vec!["restart".to_owned()],
                extras: Vec::new(),
                timeout_secs: 1200,
                moves_value: false,
                needs_solver: false,
                needs_app_wallet: true,
                needs: vec!["background".to_owned()],
            },
        ],
    }
}

/// The counts an operator wants before reading a single name.
#[test]
fn the_header_counts_the_scenarios_that_cost_sats_and_need_a_solver() {
    let text = listing().render(Style::plain(), Detail::Summary);

    assert!(text.starts_with("3 scenarios"), "{text}");
    assert!(
        text.contains("2 move sats, 1 need a dispute solver"),
        "{text}"
    );
}

/// Families are the reason the listing is readable at all: a reader looks
/// for "the dispute ones", not for a name they do not know yet.
#[test]
fn scenarios_are_grouped_under_their_family_with_a_count() {
    let text = listing().render(Style::plain(), Detail::Summary);

    assert!(text.contains("dispute (1)"), "{text}");
    assert!(text.contains("happy-path (1)"), "{text}");
    assert!(text.contains("recovery (1)"), "{text}");
    let dispute = text.find("dispute (1)").unwrap();
    let name = text.find("dispute_settled_by_solver").unwrap();
    assert!(dispute < name, "the family heads its scenarios: {text}");
}

/// A deadline is not a duration: showing `30m` next to a scenario that
/// runs in four minutes reads as "this will take half an hour". The
/// summary leaves it out; `--details` says the word "deadline" with it.
#[test]
fn deadlines_are_absent_from_the_summary_and_named_in_full_detail() {
    let summary = listing().render(Style::plain(), Detail::Summary);
    assert!(!summary.contains("30m"), "{summary}");
    assert!(!summary.contains("1800"), "{summary}");

    let full = listing().render(Style::plain(), Detail::Full);
    assert!(full.contains("deadline 30m"), "{full}");
    assert!(
        full.contains("not how long the scenario takes"),
        "the footer says what a deadline is: {full}"
    );
}

/// Whether a scenario spends real sats is the one fact that decides
/// whether it may be run against mainnet.
#[test]
fn each_scenario_carries_its_badges() {
    let text = listing().render(Style::plain(), Detail::Summary);

    let happy = line_with(&text, "happy_sell");
    assert!(happy.contains("sats"), "{happy}");
    assert!(happy.contains("smoke"), "{happy}");
    assert!(happy.contains("parity"), "{happy}");

    let restart = line_with(&text, "restart_at_pending");
    assert!(restart.contains("free"), "{restart}");
    assert!(restart.contains("app-wallet"), "{restart}");

    let solver = line_with(&text, "dispute_settled_by_solver");
    assert!(solver.contains("solver"), "{solver}");
}

/// Capabilities decide whether an app profile can run a scenario --
/// an adapter author's question, not a question for whoever is choosing
/// what to run. They belong behind `--details`.
#[test]
fn capabilities_are_only_shown_in_full_detail() {
    let summary = listing().render(Style::plain(), Detail::Summary);
    assert!(!summary.contains("onboarding"), "{summary}");
    assert!(!summary.contains("release"), "{summary}");
    assert!(!summary.contains('+'), "{summary}");

    let full = listing().render(Style::plain(), Detail::Full);
    assert_eq!(
        full.matches("onboarding").count(),
        1,
        "the shared baseline is said once: {full}"
    );
    assert!(full.contains("every scenario requires: onboarding, create-order"));
    assert!(full.contains("+ release"), "{full}");
    assert!(
        !line_with(&full, "restart_at_pending").contains('+'),
        "a scenario with no extras adds nothing: {full}"
    );
}

/// The tags are not decoration: they are what `--tag` takes, and a reader
/// who does not know that reads them as noise.
#[test]
fn the_footer_says_the_badges_and_how_to_select_what_was_read() {
    let text = listing().render(Style::plain(), Detail::Summary);

    assert!(text.contains("sats = moves real value"), "{text}");
    assert!(text.contains("free = no sats move"), "{text}");
    assert!(text.contains("mortsom run <scenario>"), "{text}");
    assert!(text.contains("mortsom run --tag smoke"), "{text}");
    assert!(
        text.lines()
            .any(|line| line.trim_end().ends_with("every scenario: mortsom run")),
        "{text}"
    );
}

/// A redirected `mortsom list` feeds pipes and logs; it stays plain.
#[test]
fn the_plain_rendering_has_no_escape_sequences() {
    let text = listing().render(Style::plain(), Detail::Summary);

    assert!(!text.contains('\u{1b}'), "{text:?}");
}

/// Colour styles what is already written; it never changes the words, so a
/// `grep` against a terminal or a pipe finds the same thing.
#[test]
fn colour_changes_only_the_styling() {
    let coloured = listing().render(Style::new(true, true), Detail::Summary);

    assert!(coloured.contains('\u{1b}'));
    assert!(
        coloured.contains(&format!("\u{1b}[{YELLOW}msats\u{1b}[0m")),
        "sats is painted: {coloured:?}"
    );
    assert_eq!(
        strip_ansi(&coloured),
        listing().render(Style::plain(), Detail::Summary)
    );
}

/// The family heading keeps its brand green on a terminal without 24-bit
/// colour, as the nearest 256-colour entry rather than an unread sequence.
#[test]
fn the_brand_green_follows_the_terminal_depth() {
    // Arrange / Act
    let truecolor = listing().render(Style::new(true, true), Detail::Summary);
    let cube = listing().render(Style::new(true, false), Detail::Summary);

    // Assert
    assert!(
        truecolor.contains("\u{1b}[38;2;155;206;75mhappy-path"),
        "{truecolor:?}"
    );
    assert!(cube.contains("\u{1b}[38;5;149mhappy-path"), "{cube:?}");
    assert!(!cube.contains("38;2;"), "{cube:?}");
}

/// Building from the registry is what the command actually does; the
/// grouping and the baseline must survive the real definitions.
#[test]
fn the_registry_builds_a_listing_with_a_baseline_and_no_other_family() {
    mortsom_scenarios::link();
    let defs = mortsom_scenario_kit::registry::all();

    let listing = Listing::from_defs(&defs);

    assert!(!listing.rows.is_empty());
    assert!(
        listing.baseline.contains(&"onboarding".to_owned()),
        "{:?}",
        listing.baseline
    );
    assert!(
        listing
            .rows
            .iter()
            .filter(|r| !r.name.starts_with("zz_runner"))
            .all(|r| r.family != "other"),
        "every scenario names a family"
    );
    assert!(
        listing
            .rows
            .iter()
            .all(|r| !r.family.is_empty() && !CROSS_CUTTING.contains(&r.family.as_str())),
        "smoke and parity label a scenario, they do not group it"
    );
    let happy = listing
        .rows
        .iter()
        .find(|r| r.name == "happy_sell")
        .expect("happy_sell is registered");
    assert_eq!(happy.family, "happy-path");
    assert!(happy.labels.contains(&"smoke".to_owned()));
    assert!(!happy.extras.contains(&"onboarding".to_owned()));
}

fn line_with<'a>(text: &'a str, needle: &str) -> &'a str {
    text.lines()
        .find(|l| l.contains(needle))
        .unwrap_or_else(|| panic!("no line with {needle} in {text}"))
}

fn strip_ansi(text: &str) -> String {
    let mut out = String::new();
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        if c != '\u{1b}' {
            out.push(c);
            continue;
        }
        for c in chars.by_ref() {
            if c == 'm' {
                break;
            }
        }
    }
    out
}

/// A scenario that needs an interruption says so in every listing: on Linux
/// and Web the run skips it, and the reader should know before asking.
#[test]
fn a_scenario_needing_an_interruption_shows_it() {
    for detail in [Detail::Summary, Detail::Full] {
        let text = listing().render(Style::plain(), detail);
        let line = text
            .lines()
            .find(|l| l.contains("restart_at_pending"))
            .unwrap();
        assert!(line.contains("interrupts:background"), "{line}");
        assert!(text.contains("interrupts = "), "{text}");
    }
}

mod catalog {
    //! What `mortsom list --json` promises a machine reader: the pull request
    //! gates that pick scenarios from it commit this output and parse it.

    use super::super::*;
    use mortsom_app_contract::Capability;
    use mortsom_scenario_kit::context::Context;
    use mortsom_scenario_kit::registry::{Need, ScenarioFuture};

    fn noop(_ctx: &Context) -> ScenarioFuture<'_> {
        Box::pin(async { Ok(()) })
    }

    static HAPPY: ScenarioDef = ScenarioDef {
        name: "happy_sell",
        tags: &["happy-path", "smoke", "parity"],
        requires: &[Capability::Onboarding, Capability::CreateOrder],
        timeout_secs: 1800,
        moves_value: true,
        needs_app_wallet: false,
        needs_solver: false,
        needs: &[],
        run: noop,
    };

    static BOND: ScenarioDef = ScenarioDef {
        name: "taker_bond_slashed_by_solver",
        tags: &["bond"],
        requires: &[Capability::Onboarding],
        timeout_secs: 2400,
        moves_value: true,
        needs_app_wallet: false,
        needs_solver: true,
        needs: &[],
        run: noop,
    };

    static BACKGROUND: ScenarioDef = ScenarioDef {
        name: "background_at_taken",
        tags: &["recovery", "interruption"],
        requires: &[],
        timeout_secs: 1200,
        moves_value: false,
        needs_app_wallet: true,
        needs_solver: false,
        needs: &[Need::Background],
        run: noop,
    };

    fn catalog() -> Catalog {
        Catalog::from_defs(&[&HAPPY, &BOND, &BACKGROUND], "9.9.9")
    }

    #[test]
    fn the_catalog_carries_its_schema_and_the_mortsom_version() {
        let c = catalog();

        assert_eq!(c.schema, CATALOG_SCHEMA);
        assert_eq!(c.mortsom_version, "9.9.9");
        assert_eq!(c.scenarios.len(), 3);
    }

    #[test]
    fn every_entry_says_what_a_gate_needs_to_select_or_skip_it() {
        let c = catalog();
        let happy = &c.scenarios[0];

        assert_eq!(happy.name, "happy_sell");
        assert_eq!(happy.family, "happy-path");
        assert_eq!(happy.tags, ["happy-path", "smoke", "parity"]);
        assert!(happy.moves_value);
        assert!(!happy.needs_bonded_daemon);
        assert_eq!(happy.timeout_secs, 1800);
        assert_eq!(happy.requires, ["onboarding", "create-order"]);
    }

    #[test]
    fn the_bond_family_is_flagged_as_needing_a_bonded_daemon() {
        let c = catalog();

        assert!(c.scenarios[1].needs_bonded_daemon);
        assert!(c.scenarios[1].needs_solver);
    }

    #[test]
    fn interruptions_and_the_app_wallet_are_listed() {
        let c = catalog();
        let bg = &c.scenarios[2];

        assert_eq!(bg.needs, ["background"]);
        assert!(bg.needs_app_wallet);
    }

    #[test]
    fn the_json_round_trips_with_stable_field_names() {
        let json = catalog().to_json().unwrap();
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();

        assert_eq!(value["schema"], CATALOG_SCHEMA);
        assert_eq!(value["scenarios"][1]["needs_bonded_daemon"], true);
        assert_eq!(value["scenarios"][2]["needs"][0], "background");
        assert!(json.ends_with('\n'), "{json:?}");
    }
}
