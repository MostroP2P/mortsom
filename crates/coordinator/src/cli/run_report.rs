//! The console report of `mortsom run`.
//!
//! A run of the whole catalogue lasts an hour or more, and its report is the
//! one thing most operators read: did every scenario work, and how long did
//! each take. So it is an aligned table — a badge, the scenario, its time —
//! and a footer with the counts, the total time and a verdict. The scenario
//! narrative (every step, every check, every payment) lives in the run's
//! artifacts, not here.
//!
//! Everything here is pure: rows are built from what each run returned and
//! rendered with a [`Style`], so a redirected report reads plain text.

use std::process::ExitCode;
use std::time::Duration;

use mortsom_scenario_kit::outcome::Outcome;

use crate::style::{self, Rgb, Style};

/// What a finished run reports about its one scenario.
pub(super) struct RunView {
    pub run_id: String,
    /// The run's own verdict, which already folds in checks and cleanup.
    pub run_passed: bool,
    pub outcome: Option<Outcome>,
    pub attempts: u32,
    /// Wall-clock of the scenario's last attempt; zero when it never ran.
    pub duration: Duration,
    /// `name: detail` of every check that failed.
    pub failed_checks: Vec<String>,
    /// Orders cleanup could not resolve.
    pub unresolved: Vec<String>,
}

/// The report of one `run` invocation: one row per selected scenario.
#[derive(Default)]
pub(super) struct Report {
    rows: Vec<Row>,
    refused: bool,
}

/// What the footer says about the invocation as a whole.
pub(super) struct Footer<'a> {
    /// Wall-clock of the whole invocation, builds and bring-ups included.
    pub elapsed: Duration,
    /// The configured artifacts directory; `None` when no run started, so
    /// the report never points at a directory nothing was written to.
    pub artifacts_dir: Option<&'a str>,
}

/// `[FLAKY]`, the widest badge, and the space after it.
const BADGE_WIDTH: usize = 7;
/// Columns consumed before a scenario name starts.
const NAME_COLUMN: usize = style::INDENT.len() + BADGE_WIDTH + 1;
/// Durations are right-aligned in a column this wide; `1h 02m 05s` fits.
const DURATION_WIDTH: usize = 10;
/// Footer labels share a column so their values line up.
const LABEL_WIDTH: usize = 9;
const SECS_PER_MINUTE: u64 = 60;
const SECS_PER_HOUR: u64 = 3600;

/// One colour per status, so a badge is recognisable by its hue before the
/// word is read. Amber and violet differ in lightness as well as hue, which
/// keeps them apart for the commonest (red/green) colour blindness.
const PASS_COLOR: Rgb = style::BRAND;
const FAIL_COLOR: Rgb = (0xe2, 0x54, 0x54);
const SKIP_COLOR: Rgb = (0x80, 0x8a, 0x96);
const FLAKY_COLOR: Rgb = (0xe6, 0xa8, 0x3a);
const NOT_RUN_COLOR: Rgb = (0xb0, 0x72, 0xd4);

struct Row {
    name: String,
    status: Status,
    duration: Duration,
    run_id: Option<String>,
    detail: Vec<String>,
}

/// How a scenario ended, as the table shows it. A pass that needed a retry
/// is flaky, not clean; a scenario a refusal kept from starting is not run,
/// which is neither a failure of the app nor a skip by design.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Status {
    Passed,
    Flaky,
    Failed,
    Skipped,
    NotRun,
}

impl Status {
    fn badge(self) -> &'static str {
        match self {
            Status::Passed => "[PASS]",
            Status::Flaky => "[FLAKY]",
            Status::Failed => "[FAIL]",
            Status::Skipped => "[SKIP]",
            Status::NotRun => "[STOP]",
        }
    }

    fn color(self) -> Rgb {
        match self {
            Status::Passed => PASS_COLOR,
            Status::Flaky => FLAKY_COLOR,
            Status::Failed => FAIL_COLOR,
            Status::Skipped => SKIP_COLOR,
            Status::NotRun => NOT_RUN_COLOR,
        }
    }
}

impl Report {
    /// Records a run that started and ended, for scenario `name`.
    pub(super) fn record(&mut self, name: &str, run: RunView) {
        let (status, detail) = classify(&run);
        self.rows.push(Row {
            name: name.to_owned(),
            status,
            duration: run.duration,
            run_id: Some(run.run_id),
            detail,
        });
    }

    /// Records a run the environment refused to start; `reason` says why.
    pub(super) fn refuse(&mut self, name: &str, reason: &str) {
        self.refused = true;
        self.rows.push(Row::not_run(name, vec![reason.to_owned()]));
    }

    /// Records scenarios a refusal kept from running.
    pub(super) fn not_run<'a>(&mut self, names: impl Iterator<Item = &'a str>) {
        self.rows
            .extend(names.map(|name| Row::not_run(name, Vec::new())));
    }

    /// How many scenarios have a row.
    pub(super) fn finished(&self) -> usize {
        self.rows.len()
    }

    /// 2 when a run was refused, 1 when a scenario failed, 0 otherwise.
    pub(super) fn code(&self) -> ExitCode {
        if self.refused {
            ExitCode::from(2)
        } else if self.count(Status::Failed) > 0 {
            ExitCode::from(1)
        } else {
            ExitCode::SUCCESS
        }
    }

    /// The table, then the counts, the total time, the artifacts and the
    /// verdict.
    pub(super) fn render(&self, footer: &Footer<'_>, style: Style) -> String {
        let rule = format!("{}{}\n", style::INDENT, style.dim(&style::rule()));
        // With one run the footer names its directory; with several, each
        // failure names its own run, the one to read or clean up.
        let runs: Vec<&str> = self
            .rows
            .iter()
            .filter_map(|r| r.run_id.as_deref())
            .collect();
        let mut out = rule.clone();
        for row in &self.rows {
            out.push_str(&row.render(runs.len() > 1, style));
        }
        out.push_str(&rule);
        out.push_str(&self.counts_line(footer.elapsed, style));
        if let Some(dir) = footer.artifacts_dir {
            let artifacts = match runs.as_slice() {
                [run] => std::path::Path::new(dir).join(run),
                _ => std::path::PathBuf::from(dir),
            };
            out.push_str(&format!(
                "{}{} {}\n",
                style::INDENT,
                style.dim(&style::pad("artifacts", LABEL_WIDTH)),
                artifacts.display()
            ));
        }
        out.push_str(&self.verdict(style));
        out
    }

    fn count(&self, status: Status) -> usize {
        self.rows.iter().filter(|row| row.status == status).count()
    }

    fn names(&self, status: Status) -> Vec<&str> {
        self.rows
            .iter()
            .filter(|row| row.status == status)
            .map(|row| row.name.as_str())
            .collect()
    }

    /// Each count in its badge's colour, so the footer reads as a tally of
    /// the table; flaky and not-run appear only when there are any.
    fn counts_line(&self, elapsed: Duration, style: Style) -> String {
        let mut cells = vec![
            style.rgb(
                PASS_COLOR,
                &format!("{} passed", self.count(Status::Passed)),
            ),
            style.rgb(
                FAIL_COLOR,
                &format!("{} failed", self.count(Status::Failed)),
            ),
            style.rgb(
                SKIP_COLOR,
                &format!("{} skipped", self.count(Status::Skipped)),
            ),
        ];
        let optional = [
            (Status::Flaky, FLAKY_COLOR, "flaky"),
            (Status::NotRun, NOT_RUN_COLOR, "not run"),
        ];
        for (status, color, label) in optional {
            let count = self.count(status);
            if count > 0 {
                cells.push(style.rgb(color, &format!("{count} {label}")));
            }
        }
        format!(
            "{}{}{}{}\n",
            style::INDENT,
            cells.join("  "),
            style.dim("  |  "),
            human_duration(elapsed)
        )
    }

    /// The verdict, then the names of what failed or never ran: on a long
    /// run those rows scrolled away long before the footer was printed.
    fn verdict(&self, style: Style) -> String {
        let failed = self.names(Status::Failed);
        let not_run = self.names(Status::NotRun);
        let mut out = String::new();
        if !failed.is_empty() {
            out.push_str(&headline(style, FAIL_COLOR, "✗", failed.len(), "failed"));
        }
        if !not_run.is_empty() {
            out.push_str(&headline(
                style,
                NOT_RUN_COLOR,
                "⚠",
                not_run.len(),
                "not run",
            ));
        }
        if out.is_empty() {
            out.push_str(&self.clean_verdict(style));
        }
        out.push_str(&name_list(style, "failed: ", &failed));
        out.push_str(&name_list(style, "not run: ", &not_run));
        out
    }

    /// "Nothing failed" is not "everything passed": a run that skipped all
    /// it selected proved nothing, and a pass that needed a retry is worth
    /// saying out loud. Each gets its own wording.
    fn clean_verdict(&self, style: Style) -> String {
        let passed = self.count(Status::Passed);
        let flaky = self.count(Status::Flaky);
        let skipped = self.count(Status::Skipped);
        let (color, text) = if passed + flaky == 0 {
            (SKIP_COLOR, format!("○ nothing ran: {skipped} skipped"))
        } else if flaky > 0 {
            let plural = if flaky == 1 { "" } else { "s" };
            let text = format!("✓ no failures, but {flaky} scenario{plural} passed only on retry");
            (FLAKY_COLOR, text)
        } else if skipped > 0 {
            (PASS_COLOR, format!("✓ no failures ({skipped} skipped)"))
        } else {
            (PASS_COLOR, "✓ all scenarios passed".to_owned())
        };
        format!("{}{}\n", style::INDENT, style.rgb(color, &text))
    }
}

impl Row {
    fn not_run(name: &str, detail: Vec<String>) -> Self {
        Self {
            name: name.to_owned(),
            status: Status::NotRun,
            duration: Duration::ZERO,
            run_id: None,
            detail,
        }
    }

    /// Badge, name and right-aligned time, then the detail wrapped under the
    /// name column.
    fn render(&self, names_run: bool, style: Style) -> String {
        let name_width = style::REPORT_WIDTH - NAME_COLUMN - DURATION_WIDTH - 1;
        let mut out = format!(
            "{}{} {} {}\n",
            style::INDENT,
            style::pad(
                &style.rgb(self.status.color(), self.status.badge()),
                BADGE_WIDTH
            ),
            style::pad(&self.name, name_width),
            style::align_right(&style.dim(&human_duration(self.duration)), DURATION_WIDTH),
        );
        let run = self
            .run_id
            .as_ref()
            .filter(|_| names_run && self.status == Status::Failed)
            .map(|id| format!("run {id}"));
        let indent = " ".repeat(NAME_COLUMN);
        for line in self
            .detail
            .iter()
            .chain(&run)
            .flat_map(|text| style::wrap(text, style::REPORT_WIDTH - NAME_COLUMN))
        {
            out.push_str(&format!("{indent}{}\n", style.dim(&line)));
        }
        out
    }
}

/// The row a finished run earns, and what it says under the name. The run's
/// own verdict wins over the scenario's: a pass whose run failed a check or
/// left an order open is a failure, and says which.
fn classify(run: &RunView) -> (Status, Vec<String>) {
    let (status, summary) = match &run.outcome {
        Some(Outcome::Passed) if run.attempts > 1 => (
            Status::Flaky,
            Some(format!("passed on attempt {}", run.attempts)),
        ),
        Some(Outcome::Passed) => (Status::Passed, None),
        Some(Outcome::Skipped { reason }) => (Status::Skipped, Some(reason.clone())),
        Some(Outcome::Failed { category, message }) => {
            (Status::Failed, Some(format!("{category}: {message}")))
        }
        None => (
            Status::Failed,
            Some("the run produced no result for it".to_owned()),
        ),
    };
    if run.run_passed {
        return (status, summary.into_iter().collect());
    }
    let first = match (status, summary) {
        (Status::Failed, Some(summary)) => summary,
        (_, Some(summary)) => format!("{summary}, but its run did not pass"),
        (_, None) => "passed, but its run did not".to_owned(),
    };
    let mut detail = vec![first];
    detail.extend(
        run.failed_checks
            .iter()
            .map(|check| format!("check {check}")),
    );
    if !run.unresolved.is_empty() {
        detail.push(format!(
            "cleanup left unresolved: {}",
            run.unresolved.join(", ")
        ));
    }
    (Status::Failed, detail)
}

/// `✗ 3 scenarios failed`, pluralised and coloured.
fn headline(style: Style, color: Rgb, mark: &str, count: usize, what: &str) -> String {
    let plural = if count == 1 { "" } else { "s" };
    let text = format!("{mark} {count} scenario{plural} {what}");
    format!("{}{}\n", style::INDENT, style.rgb(color, &text))
}

/// `failed: a, b, c`, wrapped under its label.
fn name_list(style: Style, label: &str, names: &[&str]) -> String {
    if names.is_empty() {
        return String::new();
    }
    let indent = style::INDENT.len() + label.chars().count();
    let lines = style::wrap(&names.join(", "), style::REPORT_WIDTH - indent);
    let mut out = String::new();
    for (index, line) in lines.iter().enumerate() {
        let prefix = if index == 0 {
            format!("{}{}", style::INDENT, style.dim(label))
        } else {
            " ".repeat(indent)
        };
        out.push_str(&format!("{prefix}{}\n", style.dim(line)));
    }
    out
}

/// What the invocation is about to start, said before the first run (spec
/// §10): several scenarios are several runs, each under its own spend cap.
pub(super) fn header(total: usize, moving: usize, style: Style) -> String {
    let text = if total == 1 {
        "1 scenario".to_owned()
    } else {
        format!(
            "{total} scenarios · {moving} move sats · one run each, each under its own spend cap"
        )
    };
    format!("{}{}\n", style::INDENT, style.dim(&text))
}

/// The line that marks a scenario's start in a run of several.
pub(super) fn progress(index: usize, total: usize, name: &str, style: Style) -> String {
    format!(
        "{}{}",
        style::INDENT,
        style.dim(&format!("[{index}/{total}] {name}"))
    )
}

/// `8.0s`, `4m 12s`, `1h 02m 05s`; `-` for a scenario that never ran.
/// Seconds are truncated, so a printed time is never longer than the real one.
fn human_duration(duration: Duration) -> String {
    if duration.is_zero() {
        return "-".to_owned();
    }
    let secs = duration.as_secs();
    if secs < SECS_PER_MINUTE {
        return format!("{:.1}s", duration.as_secs_f64());
    }
    let (hours, minutes, seconds) = (
        secs / SECS_PER_HOUR,
        secs % SECS_PER_HOUR / SECS_PER_MINUTE,
        secs % SECS_PER_MINUTE,
    );
    if hours == 0 {
        format!("{minutes}m {seconds:02}s")
    } else {
        format!("{hours}h {minutes:02}m {seconds:02}s")
    }
}

#[cfg(test)]
#[path = "run_report_tests.rs"]
mod tests;
