//! How `mortsom list` reads.
//!
//! The registry answers a flat list of definitions, but the operator's
//! question is never flat: "which family is this, how long does an attempt
//! take, does it move real sats, and does my profile support it?". The raw
//! dump answered the last one by repeating the seven capabilities every
//! scenario shares on every line, which pushed the differences off the
//! screen. So the rendering here groups by family, states the shared
//! baseline once, and shows per scenario only what it adds.
//!
//! Everything is pure: `render` takes the rows and a colour flag, so the
//! layout is testable and a redirected `mortsom list` stays plain text.

use std::collections::BTreeMap;

use mortsom_scenario_kit::registry::ScenarioDef;

/// Tags that label a scenario without naming its family; a scenario is
/// grouped by the first tag that is not one of these.
const CROSS_CUTTING: [&str; 2] = ["smoke", "parity"];

/// Name column; wide enough for every registered scenario today and capped
/// so one long name cannot push the rest of the line away.
const NAME_COLUMN: usize = 30;

/// Seconds in a minute, for the human duration.
const SECS_PER_MINUTE: u64 = 60;

const BOLD: &str = "1";
const DIM: &str = "2";
/// Mostro green, the hue the banner brands with.
const BRAND: &str = "38;2;155;206;75";
const YELLOW: &str = "33";
const CYAN: &str = "36";
const MAGENTA: &str = "35";

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
    /// Deadline for one attempt.
    pub timeout_secs: u64,
    /// True when the scenario moves sats.
    pub moves_value: bool,
    /// True when it needs a dispute solver registered with the daemon.
    pub needs_solver: bool,
    /// True when it drives the app's own connected wallet.
    pub needs_app_wallet: bool,
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
            })
            .collect();
        Self { rows, baseline }
    }

    /// The listing as an operator reads it, optionally styled for a
    /// terminal. Colour only ever wraps text that is already there, so a
    /// piped `mortsom list` and a terminal one say the same words.
    pub fn render(&self, color: bool) -> String {
        let paint = |code: &str, text: &str| {
            if color {
                format!("\u{1b}[{code}m{text}\u{1b}[0m")
            } else {
                text.to_owned()
            }
        };
        let mut out = String::new();
        out.push_str(&self.header(&paint));
        for (family, rows) in self.by_family() {
            out.push_str(&format!(
                "\n{} {}\n",
                paint(BRAND, &family),
                paint(DIM, &format!("({})", rows.len()))
            ));
            for row in rows {
                out.push_str(&row.render(&paint));
            }
        }
        out.push_str(&self.footer(&paint));
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

    /// What every scenario shares, plus what the badges mean.
    fn footer(&self, paint: &dyn Fn(&str, &str) -> String) -> String {
        if self.baseline.is_empty() {
            return String::new();
        }
        let mut out = format!(
            "\n{} {}\n",
            paint(DIM, "every scenario requires:"),
            paint(DIM, &self.baseline.join(", "))
        );
        out.push_str(&format!(
            "{}\n",
            paint(
                DIM,
                "badges: sats = moves real value | free = no sats move | \
                 solver = a dispute solver must be registered | app-wallet = \
                 pays through the app's own wallet | + = what this scenario \
                 needs on top of the baseline"
            )
        ));
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
    /// One scenario: name, how long an attempt may take, its badges, and
    /// what it needs beyond the baseline.
    fn render(&self, paint: &dyn Fn(&str, &str) -> String) -> String {
        // Painting adds bytes a width specifier would count, so every
        // column is padded before it is coloured.
        let padding = " ".repeat(NAME_COLUMN.saturating_sub(self.name.chars().count()));
        let duration = format!("{:>5}", human_duration(self.timeout_secs));
        let mut line = format!(
            "  {}{} {}",
            paint(BOLD, &self.name),
            padding,
            paint(DIM, &duration)
        );
        if self.moves_value {
            line.push_str(&format!("  {}", paint(YELLOW, "sats")));
        } else {
            line.push_str(&format!("  {}", paint(DIM, "free")));
        }
        if self.needs_solver {
            line.push_str(&format!(" {}", paint(MAGENTA, "solver")));
        }
        if self.needs_app_wallet {
            line.push_str(&format!(" {}", paint(MAGENTA, "app-wallet")));
        }
        for label in &self.labels {
            line.push_str(&format!(" {}", paint(CYAN, label)));
        }
        if !self.extras.is_empty() {
            line.push_str(&format!(
                "\n{}{}",
                " ".repeat(NAME_COLUMN + 2),
                paint(DIM, &format!("+ {}", self.extras.join(", ")))
            ));
        }
        line.push('\n');
        line
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
