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
            },
        ],
    }
}

/// The counts an operator wants before reading a single name.
#[test]
fn the_header_counts_the_scenarios_that_cost_sats_and_need_a_solver() {
    let text = listing().render(false);

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
    let text = listing().render(false);

    assert!(text.contains("dispute (1)"), "{text}");
    assert!(text.contains("happy-path (1)"), "{text}");
    assert!(text.contains("recovery (1)"), "{text}");
    let dispute = text.find("dispute (1)").unwrap();
    let name = text.find("dispute_settled_by_solver").unwrap();
    assert!(dispute < name, "the family heads its scenarios: {text}");
}

/// A timeout in seconds is arithmetic the reader should not have to do.
#[test]
fn timeouts_read_as_minutes() {
    let text = listing().render(false);

    assert!(text.contains("30m"), "{text}");
    assert!(text.contains("35m"), "{text}");
    assert!(!text.contains("1800s"), "{text}");
}

/// Whether a scenario spends real sats is the one fact that decides
/// whether it may be run against mainnet.
#[test]
fn each_scenario_carries_its_badges() {
    let text = listing().render(false);

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

/// The seven capabilities every scenario shares used to be repeated on
/// every line; stated once, the differences are what is left.
#[test]
fn the_shared_baseline_is_stated_once_and_only_extras_are_listed() {
    let text = listing().render(false);

    assert_eq!(
        text.matches("onboarding").count(),
        1,
        "the baseline is said once: {text}"
    );
    assert!(text.contains("every scenario requires: onboarding, create-order"));
    assert!(text.contains("free = no sats move"), "{text}");
    assert!(text.contains("+ release"), "{text}");
    assert!(
        !line_with(&text, "restart_at_pending").contains('+'),
        "a scenario with no extras gets no extras line: {text}"
    );
}

/// A redirected `mortsom list` feeds pipes and logs; it stays plain.
#[test]
fn the_plain_rendering_has_no_escape_sequences() {
    let text = listing().render(false);

    assert!(!text.contains('\u{1b}'), "{text:?}");
}

/// Colour styles what is already written; it never changes the words, so a
/// `grep` against a terminal or a pipe finds the same thing.
#[test]
fn colour_changes_only_the_styling() {
    let coloured = listing().render(true);

    assert!(coloured.contains('\u{1b}'));
    assert!(
        coloured.contains(&format!("\u{1b}[{YELLOW}msats\u{1b}[0m")),
        "sats is painted: {coloured:?}"
    );
    assert_eq!(strip_ansi(&coloured), listing().render(false));
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
