//! Scenario outcomes and results (spec §10, §12.2).

use std::time::Duration;

use mortsom_common::FailureCategory;
use serde::{Deserialize, Serialize};

/// How a scenario ended.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "outcome", rename_all = "kebab-case")]
pub enum Outcome {
    /// Every step and assertion passed.
    Passed,
    /// A step or assertion failed, with its explicit category.
    Failed {
        /// Failure category.
        category: FailureCategory,
        /// Redacted detail.
        message: String,
    },
    /// The profile lacks a required capability or a precondition was unmet;
    /// never a pass and never silently omitted.
    Skipped {
        /// Why.
        reason: String,
    },
}

impl Outcome {
    /// True when passed.
    pub fn is_passed(&self) -> bool {
        matches!(self, Outcome::Passed)
    }
}

/// Result of one scenario execution, including retries.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScenarioResult {
    /// Scenario name.
    pub name: String,
    /// App profile it ran against.
    pub app_profile: String,
    /// App version, when known.
    pub app_version: Option<String>,
    /// Final outcome.
    pub outcome: Outcome,
    /// Attempts made (1 = no retry).
    pub attempts: u32,
    /// Wall-clock duration of the last attempt.
    pub duration: Duration,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn outcome_serializes_with_a_tag() {
        let failed = Outcome::Failed {
            category: FailureCategory::Product,
            message: "wrong state".into(),
        };
        let json = serde_json::to_string(&failed).unwrap_or_default();
        assert!(json.contains("\"outcome\":\"failed\""));
        assert!(json.contains("\"category\":\"product\""));
        assert!(!failed.is_passed());
        assert!(Outcome::Passed.is_passed());
    }
}
