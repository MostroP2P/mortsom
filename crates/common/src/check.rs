//! Doctor report model and rendering. Everything here is pure.

use std::fmt;

use serde::{Deserialize, Serialize};

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

/// The complete doctor report.
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

    /// Plain-text rendering, one line per check plus a summary line.
    pub fn render(&self) -> String {
        let mut out = String::new();
        for check in &self.checks {
            out.push_str(&format!(
                "[{}] {:<28} {}\n",
                check.status, check.name, check.detail
            ));
        }
        out.push_str(&format!(
            "\n{} pass, {} warn, {} fail, {} skip -- {}\n",
            self.count(CheckStatus::Pass),
            self.count(CheckStatus::Warn),
            self.count(CheckStatus::Fail),
            self.count(CheckStatus::Skip),
            if self.is_healthy() {
                "healthy"
            } else {
                "NOT healthy"
            }
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
        report.push("appium", CheckStatus::Warn, "driver missing");
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
