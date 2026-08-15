//! Explicit failure categories (spec §10). Every failure the runner records
//! carries one of these; retries are decided from the category, never from
//! message text.

use std::fmt;

use serde::{Deserialize, Serialize};

/// Why a scenario or check failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum FailureCategory {
    /// The application under test misbehaved: wrong state, wrong copy,
    /// illegal transition.
    Product,
    /// A stable identifier or bootstrap hook the app promised is missing.
    AutomationContract,
    /// Emulator, Appium, adb, Docker or host tooling failed.
    TestInfrastructure,
    /// The run-owned relay failed or misbehaved.
    Relay,
    /// A dedicated wallet failed, rejected or timed out.
    Wallet,
    /// Configuration, secrets or prerequisites are wrong or missing.
    EnvironmentMisconfiguration,
}

impl FailureCategory {
    /// Whether one retry is permitted for this category. Only an explicitly
    /// classified infrastructure failure may be retried; UI assertions,
    /// correlation mismatches and payment mismatches never retry into a pass.
    pub fn is_retryable(self) -> bool {
        matches!(self, FailureCategory::TestInfrastructure)
    }

    /// Stable name used in reports.
    pub fn as_str(self) -> &'static str {
        match self {
            FailureCategory::Product => "product",
            FailureCategory::AutomationContract => "automation-contract",
            FailureCategory::TestInfrastructure => "test-infrastructure",
            FailureCategory::Relay => "relay",
            FailureCategory::Wallet => "wallet",
            FailureCategory::EnvironmentMisconfiguration => "environment-misconfiguration",
        }
    }
}

impl fmt::Display for FailureCategory {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn only_infrastructure_failures_are_retryable() {
        assert!(FailureCategory::TestInfrastructure.is_retryable());
        for category in [
            FailureCategory::Product,
            FailureCategory::AutomationContract,
            FailureCategory::Relay,
            FailureCategory::Wallet,
            FailureCategory::EnvironmentMisconfiguration,
        ] {
            assert!(!category.is_retryable(), "{category} must not retry");
        }
    }

    #[test]
    fn names_match_serde_representation() {
        for category in [
            FailureCategory::Product,
            FailureCategory::AutomationContract,
            FailureCategory::TestInfrastructure,
            FailureCategory::Relay,
            FailureCategory::Wallet,
            FailureCategory::EnvironmentMisconfiguration,
        ] {
            let json = serde_json::to_string(&category).unwrap();
            assert_eq!(json, format!("\"{}\"", category.as_str()));
        }
    }
}
