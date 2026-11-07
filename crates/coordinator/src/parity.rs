//! `mortsom parity <run-a> <run-b>` (spec §12.3, Phase 7.5): compares two
//! isolated runs of the shared scenario set against different application
//! profiles. It compares normalized outcome, the legal state path each
//! actor displayed, relay correlation, wallet result, cleanup result and
//! the accessibility contract (automation-contract failures). Labels,
//! navigation and screens are not compared. A difference is either listed
//! as intentional (with its reason, from an explicit file) or reported as
//! a finding.
//!
//! Every run holds one scenario (spec §8.1), so the two runs are one
//! scenario's run under each profile. A scenario counts only when both
//! summaries carry it; two runs that share none compare nothing and are
//! refused, never reported as parity.

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
    /// Profiles the difference is accepted between: it applies only when
    /// both compared profiles are listed. Empty applies to any pair.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub profiles: Vec<String>,
}

impl Intentional {
    /// Whether this entry accepts `aspect` of `scenario` between `profiles`.
    pub fn accepts(&self, scenario: &str, aspect: Aspect, profiles: (&str, &str)) -> bool {
        self.scenario == scenario
            && self.aspect == aspect
            && (self.profiles.is_empty()
                || [profiles.0, profiles.1]
                    .iter()
                    .all(|p| self.profiles.iter().any(|listed| listed == p)))
    }
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
    /// The run's recovery was abandoned: its cleanup was never verified.
    #[serde(default)]
    pub recovery_abandoned: bool,
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
    /// Scenarios both runs executed; never empty, as two runs that share
    /// none are refused.
    pub scenarios: Vec<ScenarioParity>,
    /// Scenarios only one run executed.
    pub only_in_a: Vec<String>,
    /// Scenarios only one run executed.
    pub only_in_b: Vec<String>,
    /// Runs whose recovery was abandoned; they can never reach parity.
    #[serde(default)]
    pub abandoned_runs: Vec<String>,
    /// True when at least one scenario was compared, none has an
    /// unexplained difference and no run's recovery was abandoned.
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
        if !self.abandoned_runs.is_empty() {
            out.push_str(&format!(
                "\nRuns with ABANDONED recovery (cleanup never verified): {}\n",
                self.abandoned_runs.join(", ")
            ));
        }
        let unexplained = self.scenarios.iter().any(|s| !s.findings.is_empty());
        out.push_str(if self.parity {
            "\nresult: parity (every difference is intentional)\n"
        } else if unexplained {
            "\nresult: FINDINGS (unexplained differences above)\n"
        } else {
            "\nresult: FINDINGS (a run's recovery was abandoned; its cleanup was never verified)\n"
        });
        out
    }
}

/// Compares `run_a` and `run_b` under `artifacts_dir`, writing the report
/// next to them. Every run holds one scenario (spec §8.1), so the two runs
/// are one scenario's run under each profile: a scenario counts only when
/// both summaries carry it, and two runs that share none are refused,
/// never reported as parity.
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
    let (names_a, names_b) = (scenario_names(&a), scenario_names(&b));
    if names_a.is_disjoint(&names_b) {
        return Err(format!(
            "runs {run_a} and {run_b} share no scenario ({run_a}: {}; {run_b}: {}), so nothing \
             was compared; give parity the two profiles' runs of one scenario",
            name_list(&names_a),
            name_list(&names_b)
        ));
    }
    let scenarios: Vec<ScenarioParity> = names_a
        .intersection(&names_b)
        .map(|name| compare_scenario(&a, &b, name, intentional))
        .collect();
    let abandoned_runs: Vec<String> = [(run_a, &a), (run_b, &b)]
        .into_iter()
        .filter(|(_, run)| run.abandoned)
        .map(|(id, _)| id.to_owned())
        .collect();
    let report = ParityReport {
        runs: (run_a.to_owned(), run_b.to_owned()),
        profiles: (a.summary.app_profile.clone(), b.summary.app_profile.clone()),
        parity: abandoned_runs.is_empty() && scenarios.iter().all(|s| s.findings.is_empty()),
        abandoned_runs,
        scenarios,
        only_in_a: names_a.difference(&names_b).cloned().collect(),
        only_in_b: names_b.difference(&names_a).cloned().collect(),
    };
    write_report(artifacts_dir, &report)?;
    Ok(report)
}

/// Names of the scenarios a run's summary carries.
fn scenario_names(run: &LoadedRun) -> BTreeSet<String> {
    run.summary.results.iter().map(|r| r.name.clone()).collect()
}

/// `a, b`, or `none` for a run that carries no scenario.
fn name_list(names: &BTreeSet<String>) -> String {
    if names.is_empty() {
        return "none".to_owned();
    }
    names
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>()
        .join(", ")
}

/// One scenario both runs carry: its two sides, and each differing aspect
/// either accepted by an entry for this profile pair or a finding.
fn compare_scenario(
    a: &LoadedRun,
    b: &LoadedRun,
    name: &str,
    intentional: &[Intentional],
) -> ScenarioParity {
    let pair = (
        a.summary.app_profile.as_str(),
        b.summary.app_profile.as_str(),
    );
    let side_a = side_of(a, name);
    let side_b = side_of(b, name);
    let differing = differing_aspects(&side_a, &side_b);
    let (accepted, findings): (Vec<Aspect>, Vec<Aspect>) = differing
        .into_iter()
        .partition(|aspect| intentional.iter().any(|i| i.accepts(name, *aspect, pair)));
    let intentional = accepted
        .into_iter()
        .filter_map(|aspect| {
            intentional
                .iter()
                .find(|i| i.accepts(name, aspect, pair))
                .cloned()
        })
        .collect();
    ScenarioParity {
        name: name.to_owned(),
        a: side_a,
        b: side_b,
        intentional,
        findings,
    }
}

/// Writes `parity.json` and `parity.md` under `artifacts_dir`.
fn write_report(artifacts_dir: &Path, report: &ParityReport) -> Result<(), String> {
    let json = serde_json::to_string_pretty(report).map_err(|e| e.to_string())?;
    std::fs::write(artifacts_dir.join(PARITY_JSON), json)
        .map_err(|e| format!("cannot write {PARITY_JSON}: {e}"))?;
    std::fs::write(artifacts_dir.join(PARITY_MD), report.render())
        .map_err(|e| format!("cannot write {PARITY_MD}: {e}"))
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
    abandoned: bool,
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
    Ok(LoadedRun {
        summary,
        timeline,
        abandoned: crate::recovery_abandon::abandonment(dir).is_abandoned(),
    })
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
        recovery_abandoned: run.abandoned,
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
    let clean = |side: &Side| side.unresolved_orders == 0 && !side.recovery_abandoned;
    if clean(a) != clean(b) {
        out.push(Aspect::Cleanup);
    }
    if a.contract_failure != b.contract_failure {
        out.push(Aspect::Contract);
    }
    out
}

#[cfg(test)]
#[path = "parity_tests.rs"]
mod tests;
