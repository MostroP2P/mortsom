//! Adapter errors carry an explicit failure category (spec §10).

use mortsom_common::FailureCategory;
use thiserror::Error;

/// A failure inside an adapter action.
#[derive(Debug, Error, Clone, PartialEq, Eq)]
#[error("{category}: {message}")]
pub struct AdapterError {
    /// Why it failed.
    pub category: FailureCategory,
    /// Redacted human-readable detail.
    pub message: String,
}

impl AdapterError {
    /// Builds an error.
    pub fn new(category: FailureCategory, message: impl Into<String>) -> Self {
        Self {
            category,
            message: message.into(),
        }
    }

    /// The app misbehaved (wrong state, wrong copy, illegal transition).
    pub fn product(message: impl Into<String>) -> Self {
        Self::new(FailureCategory::Product, message)
    }

    /// A promised identifier or bootstrap hook is missing.
    pub fn contract(message: impl Into<String>) -> Self {
        Self::new(FailureCategory::AutomationContract, message)
    }

    /// Emulator, Appium or tooling failed.
    pub fn infrastructure(message: impl Into<String>) -> Self {
        Self::new(FailureCategory::TestInfrastructure, message)
    }

    /// The wallet failed.
    pub fn wallet(message: impl Into<String>) -> Self {
        Self::new(FailureCategory::Wallet, message)
    }

    /// Prerequisites are wrong: the artifact under test is not the one the
    /// run expected, or configuration and secrets do not match it.
    pub fn environment(message: impl Into<String>) -> Self {
        Self::new(FailureCategory::EnvironmentMisconfiguration, message)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn constructors_set_categories() {
        assert_eq!(
            AdapterError::product("x").category,
            FailureCategory::Product
        );
        assert_eq!(
            AdapterError::contract("x").category,
            FailureCategory::AutomationContract
        );
        assert_eq!(
            AdapterError::environment("x").category,
            FailureCategory::EnvironmentMisconfiguration
        );
        assert_eq!(
            AdapterError::infrastructure("x").category,
            FailureCategory::TestInfrastructure
        );
        assert_eq!(AdapterError::wallet("x").category, FailureCategory::Wallet);
        assert_eq!(AdapterError::product("boom").to_string(), "product: boom");
    }
}
