//! The `[run]` configuration section: lane, amounts, caps and concurrency.

use mortsom_common::Lane;
use serde::{Deserialize, Serialize};

/// Absolute ceiling for `max_run_spend_sats`, regardless of configuration.
pub const MAX_RUN_SPEND_CEILING_SATS: u64 = 1_000_000;
/// At most one retry, and only for classified infrastructure failures.
pub const MAX_INFRA_RETRIES: u32 = 1;

/// `[run]` section.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunConfig {
    /// Environment lane; always explicit (spec §5).
    pub lane: Lane,
    /// Sats per order in value-moving scenarios.
    pub amount_sats: u64,
    /// Per-order cap. Required; a missing cap is an error.
    pub max_order_sats: u64,
    /// Per-run cap across all payments. Required; a missing cap is an error.
    pub max_run_spend_sats: u64,
    /// Concurrent isolated scenarios.
    #[serde(default = "default_jobs")]
    pub jobs: u32,
    /// Root directory for run artifacts.
    #[serde(default = "default_artifacts_dir")]
    pub artifacts_dir: String,
    /// Whether to run cleanup at teardown.
    #[serde(default = "default_true")]
    pub cleanup: bool,
    /// Explicit opt-in for value-moving scenarios on mainnet.
    #[serde(default)]
    pub allow_mainnet: bool,
    /// Retries permitted for classified infrastructure failures (0 or 1).
    #[serde(default = "default_infra_retries")]
    pub infra_retries: u32,
}

fn default_jobs() -> u32 {
    1
}
fn default_artifacts_dir() -> String {
    "./artifacts".to_owned()
}
fn default_true() -> bool {
    true
}
fn default_infra_retries() -> u32 {
    MAX_INFRA_RETRIES
}

impl RunConfig {
    /// Appends every problem with this section to `errors`.
    pub fn validate(&self, errors: &mut Vec<String>) {
        if self.amount_sats == 0 {
            errors.push("run.amount_sats must be greater than zero".to_owned());
        }
        if self.max_order_sats == 0 {
            errors.push("run.max_order_sats must be greater than zero".to_owned());
        }
        if self.amount_sats > self.max_order_sats {
            errors.push(format!(
                "run.amount_sats {} exceeds run.max_order_sats {}",
                self.amount_sats, self.max_order_sats
            ));
        }
        if self.max_order_sats > self.max_run_spend_sats {
            errors.push(format!(
                "run.max_order_sats {} exceeds run.max_run_spend_sats {}",
                self.max_order_sats, self.max_run_spend_sats
            ));
        }
        if self.max_run_spend_sats > MAX_RUN_SPEND_CEILING_SATS {
            errors.push(format!(
                "run.max_run_spend_sats {} exceeds the hard ceiling {}",
                self.max_run_spend_sats, MAX_RUN_SPEND_CEILING_SATS
            ));
        }
        if self.jobs == 0 {
            errors.push("run.jobs must be at least 1".to_owned());
        }
        if self.artifacts_dir.is_empty() {
            errors.push("run.artifacts_dir must not be empty".to_owned());
        }
        if self.infra_retries > MAX_INFRA_RETRIES {
            errors.push(format!(
                "run.infra_retries {} exceeds the maximum {}",
                self.infra_retries, MAX_INFRA_RETRIES
            ));
        }
        if self.lane == Lane::Canary && !self.allow_mainnet {
            errors.push("run.lane = canary requires run.allow_mainnet = true".to_owned());
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    fn valid() -> RunConfig {
        RunConfig {
            lane: Lane::Local,
            amount_sats: 1000,
            max_order_sats: 5000,
            max_run_spend_sats: 20_000,
            jobs: 1,
            artifacts_dir: "./artifacts".into(),
            cleanup: true,
            allow_mainnet: false,
            infra_retries: 1,
        }
    }

    fn errors_of(config: &RunConfig) -> Vec<String> {
        let mut errors = Vec::new();
        config.validate(&mut errors);
        errors
    }

    #[test]
    fn valid_section_has_no_errors() {
        assert!(errors_of(&valid()).is_empty());
    }

    #[test]
    fn missing_caps_fail_deserialization() {
        let result: Result<RunConfig, _> = toml::from_str("lane = \"local\"\namount_sats = 10");

        assert!(
            result.is_err(),
            "a missing cap must never default to unlimited"
        );
    }

    #[test]
    fn defaults_apply_for_optional_fields() {
        let config: RunConfig = toml::from_str(
            "lane = \"ci\"\namount_sats = 10\nmax_order_sats = 10\nmax_run_spend_sats = 10",
        )
        .unwrap();

        assert_eq!(config.jobs, 1);
        assert_eq!(config.artifacts_dir, "./artifacts");
        assert!(config.cleanup);
        assert!(!config.allow_mainnet);
        assert_eq!(config.infra_retries, 1);
    }

    #[test]
    fn cap_ordering_is_enforced() {
        let mut config = valid();
        config.amount_sats = 6000;
        config.max_run_spend_sats = 4000;

        let errors = errors_of(&config);

        assert!(
            errors
                .iter()
                .any(|e| e.contains("exceeds run.max_order_sats"))
        );
        assert!(
            errors
                .iter()
                .any(|e| e.contains("exceeds run.max_run_spend_sats"))
        );
    }

    #[test]
    fn hard_ceiling_zero_values_and_retries_are_enforced() {
        let mut config = valid();
        config.amount_sats = 0;
        config.max_order_sats = 0;
        config.max_run_spend_sats = MAX_RUN_SPEND_CEILING_SATS + 1;
        config.jobs = 0;
        config.artifacts_dir.clear();
        config.infra_retries = 2;

        let errors = errors_of(&config);

        assert!(errors.iter().any(|e| e.contains("hard ceiling")));
        assert!(errors.iter().any(|e| e.contains("run.jobs")));
        assert!(errors.iter().any(|e| e.contains("artifacts_dir")));
        assert!(errors.iter().any(|e| e.contains("infra_retries")));
        assert!(errors.iter().any(|e| e.contains("amount_sats must")));
    }

    #[test]
    fn canary_lane_requires_mainnet_opt_in() {
        let mut config = valid();
        config.lane = Lane::Canary;

        assert!(errors_of(&config).iter().any(|e| e.contains("canary")));

        config.allow_mainnet = true;
        assert!(errors_of(&config).is_empty());
    }
}
