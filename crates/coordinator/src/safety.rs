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
    /// A node did not prove it runs on regtest.
    #[error(
        "regtest refused: node {label} reports network `{network}`; only a node that reports `regtest` may move value without the mainnet guard"
    )]
    NotRegtest {
        /// Wallet label.
        label: String,
        /// Network the node reported.
        network: String,
    },
    /// No node network was verified at all.
    #[error("regtest refused: no node reported its network")]
    NoNetworkVerified,
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

/// Grants clearance for value-moving work over Mortsom-held nodes that
/// proved they run on regtest, where sats are worthless by construction.
///
/// `networks` is what each connected node answered for `get_info`, never
/// configuration: nothing infers the network, the node states it. Every
/// node must report exactly `regtest`; anything else — including an empty
/// list — is refused, so this path can never weaken the mainnet guard.
pub fn authorize_regtest(
    run: &RunConfig,
    networks: &[(String, String)],
) -> Result<MainnetClearance, SafetyError> {
    if networks.is_empty() {
        return Err(SafetyError::NoNetworkVerified);
    }
    for (label, network) in networks {
        if network != "regtest" {
            return Err(SafetyError::NotRegtest {
                label: label.clone(),
                network: network.clone(),
            });
        }
    }
    Ok(MainnetClearance { lane: run.lane })
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
            fiat_code: "USD".into(),
            fiat_amount: 1,
            payment_method: "cash".into(),
            state_timeout_secs: 1,
            settle_timeout_secs: 1,
            expiration_timeout_secs: 1,
            quiet_window_secs: 1,
            background_secs: 1,
            outage_secs: 1,
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
    fn regtest_clearance_needs_every_node_on_regtest() {
        let networks = vec![
            ("alice".to_owned(), "regtest".to_owned()),
            ("bob".to_owned(), "mainnet".to_owned()),
        ];

        let err = authorize_regtest(&run(false, Lane::Local), &networks).unwrap_err();

        assert!(
            matches!(&err, SafetyError::NotRegtest { label, network } if label == "bob" && network == "mainnet"),
            "{err}"
        );
        assert_eq!(err.category(), FailureCategory::EnvironmentMisconfiguration);
    }

    #[test]
    fn regtest_clearance_refuses_an_empty_verification() {
        let err = authorize_regtest(&run(false, Lane::Local), &[]).unwrap_err();

        assert_eq!(err, SafetyError::NoNetworkVerified);
    }

    /// Regtest sats are worthless: verified regtest nodes clear value moves
    /// without the mainnet opt-ins.
    #[test]
    fn regtest_clearance_is_granted_without_the_mainnet_guard() {
        let networks = vec![
            ("alice".to_owned(), "regtest".to_owned()),
            ("bob".to_owned(), "regtest".to_owned()),
        ];

        let clearance = authorize_regtest(&run(false, Lane::Local), &networks).unwrap();

        assert_eq!(clearance.lane, Lane::Local);
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
