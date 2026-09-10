//! `mortsom parity <run-a> <run-b>` (spec §12.3, Phase 7.5): compares two
//! isolated runs of the shared scenario set against different application
//! profiles. It compares normalized outcome, the legal state path each
//! actor displayed, relay correlation, wallet result, cleanup result and
//! the accessibility contract (automation-contract failures). Labels,
//! navigation and screens are not compared. A difference is either listed
//! as intentional (with its reason, from an explicit file) or reported as
//! a finding.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use mortsom_common::FailureCategory;
use mortsom_scenario_kit::outcome::Outcome;
use mortsom_scenario_kit::timeline::{TimelineEntry, TimelineKind};
use serde::{Deserialize, Serialize};

use crate::artifacts::files;
use crate::run::RunSummary;

/// JSON report file name (in the artifacts directory).
pub const PARITY_JSON: &str = "parity.json";
/// Markdown report file name.
pub const PARITY_MD: &str = "parity.md";

/// Aspects compared per scenario.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Aspect {
    /// Passed / failed (category) / skipped.
    Outcome,
    /// Sequence of normalized states each actor displayed.
    StatePath,
    /// Sequence of order states the relay published.
    RelayPath,
    /// Whether the run's wallets settled a payment for the scenario run.
    Wallet,
    /// Whether cleanup left the scenario's orders resolved.
    Cleanup,
    /// Automation-contract failures (missing promised identifiers).
    Contract,
}

/// One accepted difference.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Intentional {
    /// Scenario name.
    pub scenario: String,
    /// Aspect.
    pub aspect: Aspect,
    /// The product decision behind it.
    pub reason: String,
}

/// File listing intentional differences.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct IntentionalFile {
    /// Entries.
    #[serde(default)]
    pub intentional: Vec<Intentional>,
}

/// What one profile showed for one scenario.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Side {
    /// App profile.
    pub app_profile: String,
    /// App version, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub app_version: Option<String>,
    /// `passed`, `failed:<category>` or `skipped`.
    pub outcome: String,
    /// Normalized state path per actor (consecutive duplicates removed).
    pub state_path: BTreeMap<String, Vec<String>>,
    /// Relay state path.
    pub relay_path: Vec<String>,
    /// A wallet settlement was recorded.
    pub wallet_settled: bool,
    /// Orders this scenario created that cleanup left unresolved.
    pub unresolved_orders: usize,
    /// The scenario failed on the automation contract.
    pub contract_failure: bool,
}

/// Comparison of one scenario.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScenarioParity {
    /// Scenario name.
    pub name: String,
    /// Left side.
    pub a: Side,
    /// Right side.
    pub b: Side,
    /// Aspects that differ and are accepted, with reasons.
    pub intentional: Vec<Intentional>,
    /// Aspects that differ without an accepted reason.
    pub findings: Vec<Aspect>,
}

/// The parity report.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ParityReport {
    /// Run ids compared.
    pub runs: (String, String),
    /// Profiles compared.
    pub profiles: (String, String),
    /// Scenarios both runs executed.
    pub scenarios: Vec<ScenarioParity>,
    /// Scenarios only one run executed.
    pub only_in_a: Vec<String>,
    /// Scenarios only one run executed.
    pub only_in_b: Vec<String>,
    /// True when no scenario has an unexplained difference.
    pub parity: bool,
}

impl ParityReport {
    /// Markdown rendering.
    pub fn render(&self) -> String {
        let mut out = format!(
            "# Parity: {} ({}) vs {} ({})\n\n",
            self.runs.0, self.profiles.0, self.runs.1, self.profiles.1
        );
        out.push_str("| scenario | outcome A | outcome B | differs | intentional | findings |\n|---|---|---|---|---|---|\n");
        for s in &self.scenarios {
            let differs: Vec<String> = s
                .findings
                .iter()
                .chain(s.intentional.iter().map(|i| &i.aspect))
                .map(|a| format!("{a:?}"))
                .collect();
            out.push_str(&format!(
                "| {} | {} | {} | {} | {} | {} |\n",
                s.name,
                s.a.outcome,
                s.b.outcome,
                if differs.is_empty() {
                    "-".to_owned()
                } else {
                    differs.join(", ")
                },
                s.intentional
                    .iter()
                    .map(|i| format!("{:?}: {}", i.aspect, i.reason))
                    .collect::<Vec<_>>()
                    .join("; "),
                s.findings
                    .iter()
                    .map(|a| format!("{a:?}"))
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        for (label, list) in [
            ("only in A", &self.only_in_a),
            ("only in B", &self.only_in_b),
        ] {
            if !list.is_empty() {
                out.push_str(&format!("\nScenarios {label}: {}\n", list.join(", ")));
            }
        }
        for s in &self.scenarios {
            if s.a.state_path != s.b.state_path || s.a.relay_path != s.b.relay_path {
                out.push_str(&format!(
                    "\n## {}\n\nA states: {:?}\nB states: {:?}\nA relay: {:?}\nB relay: {:?}\n",
                    s.name, s.a.state_path, s.b.state_path, s.a.relay_path, s.b.relay_path
                ));
            }
        }
        out.push_str(if self.parity {
            "\nresult: parity (every difference is intentional)\n"
        } else {
            "\nresult: FINDINGS (unexplained differences above)\n"
        });
        out
    }
}

/// Compares `run_a` and `run_b` under `artifacts_dir`, writing the report
/// next to them.
pub fn compare_runs(
    artifacts_dir: &Path,
    run_a: &str,
    run_b: &str,
    intentional: &[Intentional],
) -> Result<ParityReport, String> {
    let a = load_run(&artifacts_dir.join(run_a))?;
    let b = load_run(&artifacts_dir.join(run_b))?;
    if a.summary.app_profile == b.summary.app_profile {
        return Err(format!(
            "both runs use profile `{}`; parity compares different applications",
            a.summary.app_profile
        ));
    }
    let names_a: BTreeSet<String> = a.summary.results.iter().map(|r| r.name.clone()).collect();
    let names_b: BTreeSet<String> = b.summary.results.iter().map(|r| r.name.clone()).collect();
    let mut scenarios = Vec::new();
    for name in names_a.intersection(&names_b) {
        let side_a = side_of(&a, name);
        let side_b = side_of(&b, name);
        let differing = differing_aspects(&side_a, &side_b);
        let (accepted, findings): (Vec<Aspect>, Vec<Aspect>) =
            differing.into_iter().partition(|aspect| {
                intentional
                    .iter()
                    .any(|i| i.scenario == *name && i.aspect == *aspect)
            });
        let intentional = accepted
            .into_iter()
            .filter_map(|aspect| {
                intentional
                    .iter()
                    .find(|i| i.scenario == *name && i.aspect == aspect)
                    .cloned()
            })
            .collect();
        scenarios.push(ScenarioParity {
            name: name.clone(),
            a: side_a,
            b: side_b,
            intentional,
            findings,
        });
    }
    let report = ParityReport {
        runs: (run_a.to_owned(), run_b.to_owned()),
        profiles: (a.summary.app_profile.clone(), b.summary.app_profile.clone()),
        parity: scenarios.iter().all(|s| s.findings.is_empty()),
        scenarios,
        only_in_a: names_a.difference(&names_b).cloned().collect(),
        only_in_b: names_b.difference(&names_a).cloned().collect(),
    };
    let json = serde_json::to_string_pretty(&report).map_err(|e| e.to_string())?;
    std::fs::write(artifacts_dir.join(PARITY_JSON), json)
        .map_err(|e| format!("cannot write {PARITY_JSON}: {e}"))?;
    std::fs::write(artifacts_dir.join(PARITY_MD), report.render())
        .map_err(|e| format!("cannot write {PARITY_MD}: {e}"))?;
    Ok(report)
}

/// Reads an intentional-differences file (TOML).
pub fn load_intentional(path: &Path) -> Result<Vec<Intentional>, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let file: IntentionalFile =
        toml::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(file.intentional)
}

struct LoadedRun {
    summary: RunSummary,
    timeline: Vec<TimelineEntry>,
}

fn load_run(dir: &Path) -> Result<LoadedRun, String> {
    let summary_path: PathBuf = dir.join(files::SUMMARY);
    let text = std::fs::read_to_string(&summary_path)
        .map_err(|e| format!("{}: {e}", summary_path.display()))?;
    let summary: RunSummary = serde_json::from_str(&text)
        .map_err(|e| format!("{} is not a run summary: {e}", summary_path.display()))?;
    let timeline_path = dir.join(files::TIMELINE_JSON);
    let timeline = match std::fs::read_to_string(&timeline_path) {
        Ok(text) => serde_json::from_str(&text)
            .map_err(|e| format!("{} is not a timeline: {e}", timeline_path.display()))?,
        // A run that died before writing it compares with empty paths.
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(e) => return Err(format!("{}: {e}", timeline_path.display())),
    };
    Ok(LoadedRun { summary, timeline })
}

fn side_of(run: &LoadedRun, scenario: &str) -> Side {
    let result = run.summary.results.iter().find(|r| r.name == scenario);
    let (outcome, contract_failure) = match result.map(|r| &r.outcome) {
        Some(Outcome::Passed) => ("passed".to_owned(), false),
        Some(Outcome::Failed { category, .. }) => (
            format!("failed:{category}"),
            *category == FailureCategory::AutomationContract,
        ),
        Some(Outcome::Skipped { .. }) => ("skipped".to_owned(), false),
        None => ("absent".to_owned(), false),
    };
    let section = scenario_section(&run.timeline, scenario);
    let orders = order_ids(&section);
    let unresolved_orders = run.summary.cleanup.as_ref().map_or(0, |c| {
        c.unresolved
            .iter()
            .filter(|id| orders.contains(*id))
            .count()
    });
    Side {
        app_profile: run.summary.app_profile.clone(),
        app_version: run.summary.app_version.clone(),
        outcome,
        state_path: state_paths(&section),
        relay_path: relay_path(&section),
        wallet_settled: section
            .iter()
            .any(|e| e.kind == TimelineKind::Wallet && e.summary.starts_with("wallet:")),
        unresolved_orders,
        contract_failure,
    }
}

/// Entries of the scenario's last attempt: everything between its final
/// "attempt N started" marker and the next scenario's start. Earlier
/// attempts are dropped, so a retried scenario compares by what it finally
/// did.
fn scenario_section(timeline: &[TimelineEntry], scenario: &str) -> Vec<TimelineEntry> {
    let start = format!("scenario {scenario} attempt ");
    let mut inside = false;
    let mut out = Vec::new();
    for entry in timeline {
        let is_start = entry.kind == TimelineKind::Step
            && entry.summary.starts_with("scenario ")
            && entry.summary.ends_with(" started");
        if is_start && entry.summary.starts_with(&start) {
            inside = true;
            out.clear();
            continue;
        }
        if inside && is_start {
            break;
        }
        if inside {
            out.push(entry.clone());
        }
    }
    out
}

/// Order ids the section mentions (`order <id> …`).
fn order_ids(section: &[TimelineEntry]) -> BTreeSet<String> {
    section
        .iter()
        .filter_map(|e| {
            let rest = e
                .summary
                .strip_prefix("order ")
                .or_else(|| e.summary.strip_prefix("relay: order "))
                .or_else(|| e.summary.strip_prefix("created Sell order "))
                .or_else(|| e.summary.strip_prefix("created Buy order "))?;
            rest.split_whitespace().next().map(str::to_owned)
        })
        .collect()
}

/// `UiState` entries: `order <id> shows <state> (`raw`)`, per actor,
/// consecutive duplicates removed.
fn state_paths(section: &[TimelineEntry]) -> BTreeMap<String, Vec<String>> {
    let mut paths: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for entry in section {
        if entry.kind != TimelineKind::UiState {
            continue;
        }
        let Some(state) = state_in(&entry.summary) else {
            continue;
        };
        let actor = entry.actor.clone().unwrap_or_else(|| "-".into());
        let path = paths.entry(actor).or_default();
        if path.last().map(String::as_str) != Some(state) {
            path.push(state.to_owned());
        }
    }
    paths
}

/// `relay: order <id> is <state>` entries, consecutive duplicates removed.
fn relay_path(section: &[TimelineEntry]) -> Vec<String> {
    let mut path: Vec<String> = Vec::new();
    for entry in section {
        let Some(rest) = entry.summary.strip_prefix("relay: order ") else {
            continue;
        };
        let Some((_, state)) = rest.split_once(" is ") else {
            continue;
        };
        let state = state.split_whitespace().next().unwrap_or_default();
        if path.last().map(String::as_str) != Some(state) {
            path.push(state.to_owned());
        }
    }
    path
}

/// The normalized state in `order <id> shows <state> (…)`.
fn state_in(event: &str) -> Option<&str> {
    let rest = event.strip_prefix("order ")?;
    let (_, after) = rest.split_once(" shows ")?;
    after.split_whitespace().next()
}

fn differing_aspects(a: &Side, b: &Side) -> Vec<Aspect> {
    let mut out = Vec::new();
    if a.outcome != b.outcome {
        out.push(Aspect::Outcome);
    }
    if a.state_path != b.state_path {
        out.push(Aspect::StatePath);
    }
    if a.relay_path != b.relay_path {
        out.push(Aspect::RelayPath);
    }
    if a.wallet_settled != b.wallet_settled {
        out.push(Aspect::Wallet);
    }
    if (a.unresolved_orders == 0) != (b.unresolved_orders == 0) {
        out.push(Aspect::Cleanup);
    }
    if a.contract_failure != b.contract_failure {
        out.push(Aspect::Contract);
    }
    out
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    fn summary(run_id: &str, profile: &str, outcomes: &[(&str, &str)]) -> String {
        let results: Vec<String> = outcomes
            .iter()
            .map(|(name, outcome)| {
                let o = match *outcome {
                    "passed" => r#"{"outcome": "passed"}"#.to_owned(),
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
            },
            Intentional {
                scenario: "other".into(),
                aspect: Aspect::Contract,
                reason: "same".into(),
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
    fn missing_timeline_is_empty_but_unreadable_timeline_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        write_run(dir.path(), "a", &summary("a", "profile-b", &[]), "");
        write_run(dir.path(), "b", &summary("b", "mobile-v2", &[]), "");
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
    }
}
