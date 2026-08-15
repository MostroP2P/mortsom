//! Environment lanes (spec §5). A lane is always named explicitly in
//! configuration and in every report; nothing infers mainnet.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

/// The environment lane a run executes in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Lane {
    /// Authoring and debugging on a developer machine.
    Local,
    /// Isolated relay and dedicated test infrastructure on every PR.
    Ci,
    /// Scheduled real-world integration detection with tiny hard-capped amounts.
    Canary,
}

impl Lane {
    /// The stable name used in configuration and reports.
    pub fn as_str(self) -> &'static str {
        match self {
            Lane::Local => "local",
            Lane::Ci => "ci",
            Lane::Canary => "canary",
        }
    }
}

impl fmt::Display for Lane {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for Lane {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "local" => Ok(Lane::Local),
            "ci" => Ok(Lane::Ci),
            "canary" => Ok(Lane::Canary),
            other => Err(format!(
                "unknown lane `{other}` (expected local, ci or canary)"
            )),
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn lanes_round_trip_through_names() {
        for lane in [Lane::Local, Lane::Ci, Lane::Canary] {
            assert_eq!(lane.as_str().parse::<Lane>().unwrap(), lane);
            assert_eq!(lane.to_string(), lane.as_str());
        }
    }

    #[test]
    fn unknown_lane_is_rejected() {
        assert!("mainnet".parse::<Lane>().is_err());
    }

    #[test]
    fn serde_uses_kebab_case_names() {
        let json = serde_json::to_string(&Lane::Canary).unwrap();
        assert_eq!(json, "\"canary\"");
        let lane: Lane = serde_json::from_str("\"ci\"").unwrap();
        assert_eq!(lane, Lane::Ci);
    }
}
