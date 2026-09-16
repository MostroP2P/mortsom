//! Terminal outcomes of a recovery journal and the recorded environment facts
//! that decide whether a journal may ever be abandoned.
use chrono::{DateTime, Utc};
use mortsom_common::Lane;
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, path::PathBuf};

/// Where a recovery journal stands. Only [`RecoveryOutcome::Pending`] blocks
/// new desktop work; both other states are terminal.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum RecoveryOutcome {
    /// Retained identities still need `mortsom cleanup`.
    Pending,
    /// Orders, wallets and processes were verified resolved.
    Resolved,
    /// The original Mortsom-owned regtest stack was destroyed; resolution was
    /// never verified and the run can never count as cleaned up.
    AbandonedRegtest {
        /// Why the operator closed the journal.
        reason: AbandonReason,
        /// When the terminal record was written.
        abandoned_at: DateTime<Utc>,
    },
}

impl RecoveryOutcome {
    /// Whether the journal still blocks new work.
    pub fn is_pending(&self) -> bool {
        matches!(self, Self::Pending)
    }

    /// Stable public name, used in reports.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Resolved => "resolved",
            Self::AbandonedRegtest { .. } => "abandoned_regtest",
        }
    }
}

/// The closed set of reasons a journal may be abandoned for. Free text is
/// deliberately not accepted: it could carry secrets into evidence.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, clap::ValueEnum)]
#[serde(rename_all = "kebab-case")]
pub enum AbandonReason {
    /// The stack that owned the run's orders no longer exists.
    StackDestroyed,
}

impl AbandonReason {
    /// Name used on the command line and in evidence.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::StackDestroyed => "stack-destroyed",
        }
    }
}

/// Which wallets the run's actors used.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum WalletBackend {
    /// Alice and Bob were the stack's own LND nodes.
    StackLnd,
    /// Anything else: an operator wallet, NWC, or a node outside the stack.
    External,
}

/// Facts recorded when the journal is created (and, for the node networks,
/// once the nodes answer) that abandonment is later checked against. Never
/// inferred from the lane name.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecoveryEnvironment {
    /// Lane named in configuration.
    pub lane: Lane,
    /// Whether the configuration enabled mainnet spending.
    pub allow_mainnet: bool,
    /// Canonical stack directory whose `stack up` created the stack; absent
    /// when the run did not attach to a Mortsom-owned stack.
    pub stack_dir: Option<PathBuf>,
    /// Whose wallets the actors used.
    pub wallet_backend: WalletBackend,
    /// Network each actor's node reported for itself, once connected.
    #[serde(default)]
    pub node_networks: BTreeMap<String, String>,
    /// Absolute artifacts directory the run wrote to, so abandonment evidence
    /// lands next to the run's own reports whatever configuration is
    /// selected later; absent in older manifests.
    #[serde(default)]
    pub artifacts_dir: Option<PathBuf>,
}

impl RecoveryEnvironment {
    /// Why this environment can never be abandoned through the regtest
    /// escape hatch, or `None` when every recorded fact allows it.
    pub fn abandon_refusal(&self) -> Option<String> {
        if self.lane == Lane::Canary {
            return Some("the run belongs to the canary lane".into());
        }
        if self.allow_mainnet {
            return Some("the run was configured to allow mainnet spending".into());
        }
        if self.stack_dir.is_none() {
            return Some("the run did not record a Mortsom-owned stack".into());
        }
        if self.wallet_backend != WalletBackend::StackLnd {
            return Some("the actors did not use the stack's own LND nodes".into());
        }
        for actor in ["alice", "bob"] {
            match self.node_networks.get(actor).map(String::as_str) {
                Some("regtest") => {}
                Some(other) => {
                    return Some(format!("{actor}'s node reported network `{other}`"));
                }
                None => return Some(format!("{actor}'s node network was never recorded")),
            }
        }
        None
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
pub(crate) mod tests {
    use super::*;

    pub(crate) fn regtest() -> RecoveryEnvironment {
        RecoveryEnvironment {
            lane: Lane::Local,
            allow_mainnet: false,
            stack_dir: Some("/stack".into()),
            wallet_backend: WalletBackend::StackLnd,
            node_networks: BTreeMap::from([
                ("alice".into(), "regtest".into()),
                ("bob".into(), "regtest".into()),
            ]),
            artifacts_dir: Some("/artifacts".into()),
        }
    }

    #[test]
    fn a_stack_owned_regtest_environment_may_be_abandoned() {
        assert_eq!(regtest().abandon_refusal(), None);
        let ci = RecoveryEnvironment {
            lane: Lane::Ci,
            ..regtest()
        };
        assert_eq!(ci.abandon_refusal(), None);
    }

    #[test]
    fn canary_mainnet_external_and_unverified_environments_are_refused() {
        let cases = [
            RecoveryEnvironment {
                lane: Lane::Canary,
                ..regtest()
            },
            RecoveryEnvironment {
                allow_mainnet: true,
                ..regtest()
            },
            RecoveryEnvironment {
                stack_dir: None,
                ..regtest()
            },
            RecoveryEnvironment {
                wallet_backend: WalletBackend::External,
                ..regtest()
            },
            RecoveryEnvironment {
                node_networks: BTreeMap::from([
                    ("alice".into(), "regtest".into()),
                    ("bob".into(), "mainnet".into()),
                ]),
                ..regtest()
            },
            RecoveryEnvironment {
                node_networks: BTreeMap::from([("alice".into(), "regtest".into())]),
                ..regtest()
            },
        ];
        for case in cases {
            assert!(case.abandon_refusal().is_some(), "{case:?}");
        }
    }

    #[test]
    fn outcomes_serialize_with_a_state_tag_and_only_pending_blocks() {
        let abandoned = RecoveryOutcome::AbandonedRegtest {
            reason: AbandonReason::StackDestroyed,
            abandoned_at: DateTime::from_timestamp(1, 0).unwrap(),
        };
        let json = serde_json::to_value(&abandoned).unwrap();
        assert_eq!(json["state"], "abandoned_regtest");
        assert_eq!(json["reason"], "stack-destroyed");
        assert!(RecoveryOutcome::Pending.is_pending());
        assert!(!RecoveryOutcome::Resolved.is_pending());
        assert!(!abandoned.is_pending());
    }
}
