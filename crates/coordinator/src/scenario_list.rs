//! How `mortsom list` reads.
//!
//! The registry answers a flat list of definitions, but the operator's
//! question is never flat: "which scenarios are there, what do they cover,
//! which ones spend real sats, and how do I select them?". The raw dump
//! answered none of those directly: it repeated on every line the seven
//! capabilities every scenario shares, and it printed each scenario's
//! timeout, which is the deadline that aborts a stuck attempt and not how
//! long the scenario takes -- a reader has no way to tell those apart from
//! a bare `1800s`.
//!
//! So the default listing groups by family, badges only what changes a
//! decision (sats, solver, app wallet) and states the tags as what they
//! are: what `--tag` selects. Capabilities and deadlines are what someone
//! writing an app adapter needs, not what someone choosing a scenario
//! needs, so `--details` carries them.
//!
//! Everything is pure: `render` takes the rows and a colour flag, so the
//! layout is testable and a redirected `mortsom list` stays plain text.

use std::collections::BTreeMap;

use mortsom_scenario_kit::registry::ScenarioDef;
use serde::Serialize;

use crate::run::BOND_FAMILY;
use crate::style::{self, Style};

/// Tags that label a scenario without naming its family; a scenario is
/// grouped by the first tag that is not one of these.
const CROSS_CUTTING: [&str; 2] = ["smoke", "parity"];

/// Name column; wide enough for every registered scenario today and capped
/// so one long name cannot push the rest of the line away.
const NAME_COLUMN: usize = 30;

/// Seconds in a minute, for the human deadline.
const SECS_PER_MINUTE: u64 = 60;

const BOLD: &str = "1";
const DIM: &str = "2";
const YELLOW: &str = "33";
const CYAN: &str = "36";
const MAGENTA: &str = "35";

/// What the listing shows beyond the names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Detail {
    /// What choosing a scenario needs: family, badges, tags.
    Summary,
    /// Also the capabilities an app profile must support and the deadline
    /// that aborts a stuck attempt.
    Full,
}

impl Detail {
    /// Whether capabilities and deadlines are shown.
    fn is_full(self) -> bool {
        self == Detail::Full
    }
}

/// One scenario as the listing needs it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    /// Scenario name, the argument `mortsom run` takes.
    pub name: String,
    /// Family tag the scenario is grouped under.
    pub family: String,
    /// Remaining tags, shown as labels.
    pub labels: Vec<String>,
    /// Capabilities this scenario needs beyond the shared baseline.
    pub extras: Vec<String>,
    /// Deadline that aborts one attempt; not how long the scenario takes.
    pub timeout_secs: u64,
    /// True when the scenario moves sats.
    pub moves_value: bool,
    /// True when it needs a dispute solver registered with the daemon.
    pub needs_solver: bool,
    /// True when it drives the app's own connected wallet.
    pub needs_app_wallet: bool,
    /// Interruptions it causes outside the app; Linux and Web runs skip it.
    pub needs: Vec<String>,
}

/// The rows and what every one of them shares.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Listing {
    /// Scenarios, in registry order.
    pub rows: Vec<Row>,
    /// Capabilities every scenario requires; stated once instead of on
    /// every line.
    pub baseline: Vec<String>,
}

impl Listing {
    /// Builds the listing from registry definitions.
    pub fn from_defs(defs: &[&'static ScenarioDef]) -> Self {
        let baseline = shared_capabilities(defs);
        let rows = defs
            .iter()
            .map(|def| Row {
                name: def.name.to_owned(),
                family: family_of(def.tags).to_owned(),
                labels: def
                    .tags
                    .iter()
                    .filter(|t| **t != family_of(def.tags))
                    .map(|t| (*t).to_owned())
                    .collect(),
                extras: def
                    .requires
                    .iter()
                    .map(|c| c.as_str().to_owned())
                    .filter(|c| !baseline.contains(c))
                    .collect(),
                timeout_secs: def.timeout_secs,
                moves_value: def.moves_value,
                needs_solver: def.needs_solver,
                needs_app_wallet: def.needs_app_wallet,
                needs: def.needs.iter().map(|n| n.as_str().to_owned()).collect(),
            })
            .collect();
        Self { rows, baseline }
    }

    /// The listing as an operator reads it, optionally styled for a
    /// terminal. Colour only ever wraps text that is already there, so a
    /// piped `mortsom list` and a terminal one say the same words.
    pub fn render(&self, style: Style, detail: Detail) -> String {
        let paint = |code: &str, text: &str| style.paint(code, text);
        // Mostro green, the hue the banner brands with, at the depth the
        // terminal can show.
        let brand = style.rgb_code(style::BRAND);
        let mut out = String::new();
        out.push_str(&self.header(&paint));
        for (family, rows) in self.by_family() {
            out.push_str(&format!(
                "\n{} {}\n",
                paint(&brand, &family),
                paint(DIM, &format!("({})", rows.len()))
            ));
            for row in rows {
                out.push_str(&row.render(&paint, detail));
            }
        }
        out.push_str(&self.footer(&paint, detail));
        out
    }

    /// The counts a reader wants before any name: how many scenarios there
    /// are, and how many of them cost sats or need a solver.
    fn header(&self, paint: &dyn Fn(&str, &str) -> String) -> String {
        let value = self.rows.iter().filter(|r| r.moves_value).count();
        let solver = self.rows.iter().filter(|r| r.needs_solver).count();
        format!(
            "{} {}\n",
            paint(BOLD, &format!("{} scenarios", self.rows.len())),
            paint(
                DIM,
                &format!("- {value} move sats, {solver} need a dispute solver")
            )
        )
    }

    /// What the badges mean, how to select what you just read, and -- in
    /// full detail -- the capabilities every scenario shares.
    fn footer(&self, paint: &dyn Fn(&str, &str) -> String, detail: Detail) -> String {
        let mut out = format!(
            "\n{}\n",
            paint(
                DIM,
                "sats = moves real value | free = no sats move | solver = a \
                 dispute solver must be registered | app-wallet = pays \
                 through the app's own wallet | interrupts = needs an \
                 interruption Linux and Web runs cannot cause, so they skip it"
            )
        );
        out.push_str(&format!(
            "{}\n",
            paint(
                DIM,
                "run one by name: mortsom run <scenario> -- a whole set by \
                 any word above: mortsom run --tag smoke -- or every \
                 scenario: mortsom run"
            )
        ));
        if detail.is_full() && !self.baseline.is_empty() {
            out.push_str(&format!(
                "\n{} {}\n",
                paint(DIM, "every scenario requires:"),
                paint(DIM, &self.baseline.join(", "))
            ));
            out.push_str(&format!(
                "{}\n",
                paint(
                    DIM,
                    "+ = capabilities beyond that baseline; deadline = when a \
                     stuck attempt is aborted, not how long the scenario takes"
                )
            ));
        }
        out
    }

    /// Rows grouped by family, families in alphabetical order.
    fn by_family(&self) -> Vec<(String, Vec<&Row>)> {
        let mut grouped: BTreeMap<String, Vec<&Row>> = BTreeMap::new();
        for row in &self.rows {
            grouped.entry(row.family.clone()).or_default().push(row);
        }
        grouped.into_iter().collect()
    }
}

impl Row {
    /// One scenario: its name, the badges that decide whether it may be
    /// run, and the tags that select it. With [`Detail::Full`], also what
    /// the app profile must support and the deadline for one attempt.
    fn render(&self, paint: &dyn Fn(&str, &str) -> String, detail: Detail) -> String {
        // Painting adds bytes a width specifier would count, so the name is
        // padded before it is coloured.
        let padding = " ".repeat(NAME_COLUMN.saturating_sub(self.name.chars().count()));
        let mut line = format!("  {}{}", paint(BOLD, &self.name), padding);
        if self.moves_value {
            line.push_str(&format!(" {}", paint(YELLOW, "sats")));
        } else {
            line.push_str(&format!(" {}", paint(DIM, "free")));
        }
        if self.needs_solver {
            line.push_str(&format!(" {}", paint(MAGENTA, "solver")));
        }
        if self.needs_app_wallet {
            line.push_str(&format!(" {}", paint(MAGENTA, "app-wallet")));
        }
        if !self.needs.is_empty() {
            let needs = format!("interrupts:{}", self.needs.join(","));
            line.push_str(&format!(" {}", paint(MAGENTA, &needs)));
        }
        for label in &self.labels {
            line.push_str(&format!(" {}", paint(CYAN, label)));
        }
        if detail.is_full() {
            line.push_str(&self.detail_line(paint));
        }
        line.push('\n');
        line
    }

    /// The adapter author's line: extra capabilities and the deadline.
    fn detail_line(&self, paint: &dyn Fn(&str, &str) -> String) -> String {
        let mut detail = format!("deadline {}", human_duration(self.timeout_secs));
        if !self.extras.is_empty() {
            detail = format!("+ {} | {detail}", self.extras.join(", "));
        }
        format!("\n{}{}", " ".repeat(NAME_COLUMN + 2), paint(DIM, &detail))
    }
}

/// Version of the `mortsom list --json` document. Bumped on any change a
/// reader would have to handle: a removed or renamed field, a new meaning.
pub const CATALOG_SCHEMA: u32 = 1;

/// The registry as a machine reads it (`mortsom list --json`).
///
/// Pull request gates commit this document and select scenarios from it,
/// so it carries everything selection and skipping depend on and nothing
/// that is only layout.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Catalog {
    /// [`CATALOG_SCHEMA`].
    pub schema: u32,
    /// Version of the Mortsom that produced it.
    pub mortsom_version: String,
    /// Scenarios, in registry order.
    pub scenarios: Vec<CatalogEntry>,
}

/// One scenario of the [`Catalog`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CatalogEntry {
    /// Name `mortsom run` takes.
    pub name: String,
    /// Family the scenario is grouped under.
    pub family: String,
    /// Every tag, in registry order (the family is not necessarily first);
    /// `--tag` selects by any.
    pub tags: Vec<String>,
    /// True when the scenario moves sats.
    pub moves_value: bool,
    /// True when it needs a dispute solver registered with the daemon.
    pub needs_solver: bool,
    /// True when it drives the app's own connected wallet; a run whose
    /// payer is a node Mortsom holds skips it.
    pub needs_app_wallet: bool,
    /// True when it is written for a daemon with anti-abuse bonds
    /// (`stack up --bond`); every other scenario is skipped on one.
    pub needs_bonded_daemon: bool,
    /// Interruptions it causes outside the app; Linux and Web runs skip it.
    pub needs: Vec<String>,
    /// Capabilities the app profile must support.
    pub requires: Vec<String>,
    /// Deadline that aborts one attempt; not how long the scenario takes.
    pub timeout_secs: u64,
}

impl Catalog {
    /// Builds the catalog from registry definitions.
    pub fn from_defs(defs: &[&'static ScenarioDef], mortsom_version: &str) -> Self {
        let scenarios = defs
            .iter()
            .map(|def| CatalogEntry {
                name: def.name.to_owned(),
                family: family_of(def.tags).to_owned(),
                tags: def.tags.iter().map(|t| (*t).to_owned()).collect(),
                moves_value: def.moves_value,
                needs_solver: def.needs_solver,
                needs_app_wallet: def.needs_app_wallet,
                needs_bonded_daemon: def.tags.contains(&BOND_FAMILY),
                needs: def.needs.iter().map(|n| n.as_str().to_owned()).collect(),
                requires: def.requires.iter().map(|c| c.as_str().to_owned()).collect(),
                timeout_secs: def.timeout_secs,
            })
            .collect();
        Self {
            schema: CATALOG_SCHEMA,
            mortsom_version: mortsom_version.to_owned(),
            scenarios,
        }
    }

    /// Pretty JSON with a trailing newline, so a committed copy diffs
    /// cleanly.
    pub fn to_json(&self) -> Result<String, serde_json::Error> {
        let mut json = serde_json::to_string_pretty(self)?;
        json.push('\n');
        Ok(json)
    }
}

/// The family a scenario belongs to: its first tag that names one.
fn family_of(tags: &[&'static str]) -> &'static str {
    tags.iter()
        .find(|t| !CROSS_CUTTING.contains(t))
        .copied()
        .unwrap_or("other")
}

/// Capabilities every scenario requires, in the order a scenario lists
/// them so the baseline reads like a run does.
///
/// A scenario that requires nothing -- the runner's own self-tests -- asks
/// for no capability and so cannot shrink what the rest share; counting it
/// would collapse the baseline to nothing and put all seven shared
/// capabilities back on every line.
fn shared_capabilities(defs: &[&'static ScenarioDef]) -> Vec<String> {
    let demanding: Vec<&&ScenarioDef> = defs.iter().filter(|d| !d.requires.is_empty()).collect();
    let Some(first) = demanding.first() else {
        return Vec::new();
    };
    first
        .requires
        .iter()
        .map(|c| c.as_str().to_owned())
        .filter(|cap| {
            demanding
                .iter()
                .all(|def| def.requires.iter().any(|c| c.as_str() == cap))
        })
        .collect()
}

/// A deadline as an operator thinks of it: whole minutes, or seconds when
/// it is under a minute.
fn human_duration(secs: u64) -> String {
    if secs < SECS_PER_MINUTE {
        return format!("{secs}s");
    }
    format!("{}m", secs / SECS_PER_MINUTE)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
#[path = "scenario_list_tests.rs"]
mod tests;
