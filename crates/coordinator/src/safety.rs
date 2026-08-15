//! Global safety guard: value-moving work on mainnet requires explicit
//! configuration opt-in **and** a confirmation environment variable. Nothing
//! infers mainnet; nothing value-moving runs without both.

use mortsom_common::{FailureCategory, Lane};
use thiserror::Error;

use crate::config::RunConfig;

/// Environment variable that confirms mainnet spending for this process.
pub const MAINNET_CONFIRM_ENV: &str = "MORTSOM_MAINNET_CONFIRM";
/// Exact value the confirmation variable must hold.
pub const MAINNET_CONFIRM_VALUE: &str = "I_ACCEPT_MAINNET_SPEND";

/// Why value-moving work was refused.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum SafetyError {
    /// `run.allow_mainnet` is false.
    #[error("mainnet refused: run.allow_mainnet is false (lane {lane})")]
    NotAllowed {
        /// Configured lane.
        lane: Lane,
    },
    /// The confirmation variable is missing or wrong.
    #[error(
        "mainnet refused: set {MAINNET_CONFIRM_ENV}={MAINNET_CONFIRM_VALUE} to confirm (lane {lane})"
    )]
    NotConfirmed {
        /// Configured lane.
        lane: Lane,
    },
}

impl SafetyError {
    /// Every refusal is an environment misconfiguration, never a product bug.
    pub fn category(&self) -> FailureCategory {
        FailureCategory::EnvironmentMisconfiguration
    }
}

/// Proof that mainnet spending was explicitly authorized. Only obtainable
/// through [`authorize_mainnet`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MainnetClearance {
    /// Lane the clearance was granted for.
    pub lane: Lane,
}

/// Grants clearance for value-moving work, or refuses with the reason.
pub fn authorize_mainnet<F>(run: &RunConfig, env: F) -> Result<MainnetClearance, SafetyError>
where
    F: Fn(&str) -> Option<String>,
{
    if !run.allow_mainnet {
        return Err(SafetyError::NotAllowed { lane: run.lane });
    }
    match env(MAINNET_CONFIRM_ENV) {
        Some(value) if value == MAINNET_CONFIRM_VALUE => Ok(MainnetClearance { lane: run.lane }),
        _ => Err(SafetyError::NotConfirmed { lane: run.lane }),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    fn run(allow_mainnet: bool, lane: Lane) -> RunConfig {
        RunConfig {
            lane,
            amount_sats: 1,
            max_order_sats: 1,
            max_run_spend_sats: 1,
            jobs: 1,
            artifacts_dir: "a".into(),
            cleanup: true,
            allow_mainnet,
            infra_retries: 0,
        }
    }

    #[test]
    fn refuses_when_config_does_not_allow_mainnet_even_if_confirmed() {
        let err = authorize_mainnet(&run(false, Lane::Local), |_| {
            Some(MAINNET_CONFIRM_VALUE.into())
        })
        .unwrap_err();

        assert_eq!(err, SafetyError::NotAllowed { lane: Lane::Local });
        assert_eq!(err.category(), FailureCategory::EnvironmentMisconfiguration);
    }

    #[test]
    fn refuses_without_confirmation_variable() {
        let err = authorize_mainnet(&run(true, Lane::Canary), |_| None).unwrap_err();

        assert_eq!(err, SafetyError::NotConfirmed { lane: Lane::Canary });
        assert!(err.to_string().contains(MAINNET_CONFIRM_ENV));
    }

    #[test]
    fn refuses_wrong_confirmation_value() {
        let err = authorize_mainnet(&run(true, Lane::Ci), |_| Some("yes".into())).unwrap_err();

        assert_eq!(err, SafetyError::NotConfirmed { lane: Lane::Ci });
    }

    #[test]
    fn grants_clearance_with_both_opt_ins() {
        let clearance = authorize_mainnet(&run(true, Lane::Canary), |name| {
            (name == MAINNET_CONFIRM_ENV).then(|| MAINNET_CONFIRM_VALUE.to_owned())
        })
        .unwrap();

        assert_eq!(clearance.lane, Lane::Canary);
    }
}
