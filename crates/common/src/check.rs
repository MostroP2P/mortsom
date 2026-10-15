//! Check results shared by `mortsom doctor` and pre-flight: status, named
//! result and a renderable report. Everything here is pure.

use std::fmt;

use serde::{Deserialize, Serialize};

/// Narrowest name column; keeps short reports from looking ragged.
const NAME_COLUMN_MIN: usize = 28;
/// Widest name column; a single long name must not push the details away.
const NAME_COLUMN_MAX: usize = 44;

/// Outcome of one check.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CheckStatus {
    /// Requirement met.
    Pass,
    /// Usable, but worth attention.
    Warn,
    /// Requirement not met; a run would fail or be unsafe.
    Fail,
    /// Not evaluated because a prerequisite failed or was disabled.
    Skip,
}

impl fmt::Display for CheckStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            CheckStatus::Pass => "PASS",
            CheckStatus::Warn => "WARN",
            CheckStatus::Fail => "FAIL",
            CheckStatus::Skip => "SKIP",
        })
    }
}

impl CheckStatus {
    /// SGR parameters used when the output goes to a terminal: green for a
    /// requirement met, red for one that is not, yellow for attention and
    /// grey for what was never evaluated.
    pub fn ansi_code(self) -> &'static str {
        match self {
            CheckStatus::Pass => "32",
            CheckStatus::Warn => "33",
            CheckStatus::Fail => "31",
            CheckStatus::Skip => "90",
        }
    }
}

/// One named check with a human-readable, already redacted detail.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheckResult {
    /// Stable check name, e.g. `docker`, `env MORTSOM_ALICE_NWC`.
    pub name: String,
    /// Outcome.
    pub status: CheckStatus,
    /// Detail; must never contain a secret.
    pub detail: String,
}

/// A list of checks with a health verdict.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DoctorReport {
    /// Checks in execution order.
    pub checks: Vec<CheckResult>,
}

impl DoctorReport {
    /// Appends a check.
    pub fn push(
        &mut self,
        name: impl Into<String>,
        status: CheckStatus,
        detail: impl Into<String>,
    ) {
        self.checks.push(CheckResult {
            name: name.into(),
            status,
            detail: detail.into(),
        });
    }

    /// True when no check failed.
    pub fn is_healthy(&self) -> bool {
        !self.checks.iter().any(|c| c.status == CheckStatus::Fail)
    }

    /// Number of checks with the given status.
    pub fn count(&self, status: CheckStatus) -> usize {
        self.checks.iter().filter(|c| c.status == status).count()
    }

    /// Plain-text rendering, one line per check plus a summary line. This is
    /// what goes into artifacts and what scripts read, so it never carries
    /// escape sequences.
    pub fn render(&self) -> String {
        self.render_with_color(false)
    }

    /// [`render`](Self::render), optionally styled for a terminal. Colour
    /// wraps the status word and the verdict only, so the text itself — and
    /// anything grepping for it — is unchanged.
    pub fn render_with_color(&self, color: bool) -> String {
        let paint = |text: &str, code: &str| {
            if color {
                format!("\u{1b}[{code}m{text}\u{1b}[0m")
            } else {
                text.to_owned()
            }
        };
        // Details line up with the longest name so the eye can run down the
        // column, but one very long name does not push every detail off the
        // screen.
        let width = self
            .checks
            .iter()
            .map(|c| c.name.chars().count())
            .max()
            .unwrap_or(0)
            .clamp(NAME_COLUMN_MIN, NAME_COLUMN_MAX);
        let mut out = String::new();
        for check in &self.checks {
            let status = check.status.to_string();
            let painted = paint(&status, check.status.ansi_code());
            out.push_str(&format!(
                "[{painted}] {:<width$} {}\n",
                check.name, check.detail
            ));
        }
        let (verdict, verdict_code) = if self.is_healthy() {
            ("healthy", "1;32")
        } else {
            ("NOT healthy", "1;31")
        };
        out.push_str(&format!(
            "\n{} pass, {} warn, {} fail, {} skip -- {}\n",
            self.count(CheckStatus::Pass),
            self.count(CheckStatus::Warn),
            self.count(CheckStatus::Fail),
            self.count(CheckStatus::Skip),
            paint(verdict, verdict_code)
        ));
        out
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn healthy_when_no_failures() {
        let mut report = DoctorReport::default();
        report.push("docker", CheckStatus::Pass, "29.7");
        report.push("chromedriver", CheckStatus::Warn, "old driver");
        report.push("wallet bob", CheckStatus::Skip, "no network");

        assert!(report.is_healthy());
        assert_eq!(report.count(CheckStatus::Warn), 1);
    }

    #[test]
    fn any_failure_makes_it_unhealthy_and_render_shows_summary() {
        let mut report = DoctorReport::default();
        report.push("env MORTSOM_ALICE_NWC", CheckStatus::Fail, "not set");

        let text = report.render();

        assert!(!report.is_healthy());
        assert!(text.contains("[FAIL] env MORTSOM_ALICE_NWC"));
        assert!(text.contains("0 pass, 0 warn, 1 fail, 0 skip -- NOT healthy"));
    }

    #[test]
    fn json_serialization_uses_lowercase_status() {
        let mut report = DoctorReport::default();
        report.push("java", CheckStatus::Pass, "17");

        let json = serde_json::to_string(&report).unwrap();

        assert!(json.contains("\"status\":\"pass\""));
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod color_tests {
    use super::*;

    fn report() -> DoctorReport {
        let mut report = DoctorReport::default();
        report.push("docker", CheckStatus::Pass, "29.7");
        report.push("wallet bob", CheckStatus::Fail, "empty");
        report.push("chromedriver", CheckStatus::Warn, "old");
        report.push("network", CheckStatus::Skip, "no relay");
        report
    }

    /// Evidence written to artifacts and read by scripts must never carry
    /// escape sequences, so plain stays the default.
    #[test]
    fn the_plain_rendering_has_no_escape_sequences() {
        let plain = report().render();
        assert!(!plain.contains('\u{1b}'), "{plain:?}");
        assert!(plain.contains("[PASS] docker"));
    }

    /// Colour is added around the status word only: the text a reader (or a
    /// `grep`) looks for is unchanged.
    #[test]
    fn colour_wraps_each_status_without_changing_the_text() {
        let coloured = report().render_with_color(true);
        for (status, code) in [
            ("PASS", "32"),
            ("FAIL", "31"),
            ("WARN", "33"),
            ("SKIP", "90"),
        ] {
            assert!(
                coloured.contains(&format!("\u{1b}[{code}m{status}\u{1b}[0m")),
                "{status} is not wrapped in {code}: {coloured:?}"
            );
        }
        let stripped: String = strip_ansi(&coloured);
        assert_eq!(stripped, report().render(), "only styling may differ");
    }

    /// The verdict is the line a tired operator looks at first.
    #[test]
    fn the_verdict_is_coloured_by_health() {
        let unhealthy = report().render_with_color(true);
        assert!(
            unhealthy.contains("\u{1b}[1;31mNOT healthy\u{1b}[0m"),
            "{unhealthy:?}"
        );

        let mut healthy = DoctorReport::default();
        healthy.push("docker", CheckStatus::Pass, "29.7");
        let healthy = healthy.render_with_color(true);
        assert!(
            healthy.contains("\u{1b}[1;32mhealthy\u{1b}[0m"),
            "{healthy:?}"
        );
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
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod layout_tests {
    use super::*;

    #[test]
    fn details_line_up_and_a_very_long_name_does_not_push_them_away() {
        let mut report = DoctorReport::default();
        report.push("docker", CheckStatus::Pass, "29.7");
        report.push(
            "relay ws://host.docker.internal:7500",
            CheckStatus::Skip,
            "idle",
        );
        let rendered = report.render();

        let columns: Vec<usize> = rendered
            .lines()
            .filter(|l| l.starts_with('['))
            .map(|l| l.find("29.7").or_else(|| l.find("idle")).unwrap())
            .collect();
        assert_eq!(columns[0], columns[1], "details share a column: {rendered}");

        // A name past the maximum keeps its own line readable instead of
        // widening every other one.
        let mut wide = DoctorReport::default();
        wide.push("x".repeat(80), CheckStatus::Pass, "detail");
        assert!(
            wide.render()
                .contains(&format!("{} detail", "x".repeat(80)))
        );
    }
}
