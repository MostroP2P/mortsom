//! Mortsom configuration: a committed non-secret baseline plus an ignored
//! local overlay and `env:` references for every secret (spec §16.1).

mod load;
mod maintenance;
mod publication;
mod run;
mod source;
mod ui;
mod validate;

use mortsom_app_contract::MobileConfig;
use mortsom_common::MissingEnv;
use mortsom_environment::{MostroConfig, RelayConfig, WalletsConfig};
use serde::{Deserialize, Serialize};
use thiserror::Error;

pub use load::{ConfigDiagnosis, LOCAL_OVERLAY_SUFFIX, configured_profile};
pub use maintenance::{ArtifactSettings, MaintenanceConfig};
pub use publication::PublishSettings;
pub use run::{MAX_RUN_SPEND_CEILING_SATS, RunConfig};
pub use source::AppSource;
pub use ui::UiConfig;

/// The complete, resolved and validated Mortsom configuration.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    /// `[mobile]` — application checkout and profile.
    pub mobile: MobileConfig,
    /// `[ui]` — shared interaction deadlines, independent of client platform.
    #[serde(default)]
    pub ui: UiConfig,
    /// `[relay]` — the stack relay port, and the run-owned relay of `relay smoke`.
    #[serde(default = "default_relay")]
    pub relay: RelayConfig,
    /// `[mostro]` — external local daemon.
    pub mostro: MostroConfig,
    /// `[wallets]` — dedicated NWC wallets.
    pub wallets: WalletsConfig,
    /// `[run]` — lane, amounts, caps, concurrency, artifacts.
    pub run: RunConfig,
}

/// Deadline for one UI interaction unless `[ui].timeout_secs` says otherwise.
pub const DEFAULT_UI_TIMEOUT_SECS: u64 = 30;

impl Config {
    /// The configured interaction deadline, or the default.
    pub fn ui_timeout_secs(&self) -> u64 {
        self.ui.timeout_secs.unwrap_or(DEFAULT_UI_TIMEOUT_SECS)
    }
}

fn default_relay() -> RelayConfig {
    // Every field of `[relay]` has a default, so an absent section is valid.
    RelayConfig::default()
}

/// Configuration loading failures. Every variant fails closed: nothing is
/// started until the configuration is complete and valid.
#[derive(Debug, Error)]
pub enum ConfigError {
    /// A configuration file could not be read.
    #[error("cannot read config file {path}: {source}")]
    Read {
        /// File that failed.
        path: String,
        /// Underlying I/O error.
        #[source]
        source: std::io::Error,
    },
    /// The TOML could not be parsed or does not match the schema.
    #[error("invalid config: {0}")]
    Parse(String),
    /// One or more `env:` references were not set.
    #[error(transparent)]
    MissingEnv(#[from] MissingEnv),
    /// A secret was written literally in TOML instead of via `env:`.
    #[error("{key} must be an `env:` reference; secrets never live in TOML")]
    SecretInToml {
        /// Dotted configuration key.
        key: String,
    },
    /// Validation problems, all of them at once.
    #[error("invalid config: {}", .0.join("; "))]
    Invalid(Vec<String>),
}
