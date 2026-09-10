//! Mortsom coordinator: configuration, runner, budgets, timeouts, cleanup,
//! reporting and the `mortsom` command-line interface.

pub mod accounting;
pub mod apk;
pub mod app_setup;
pub mod artifacts;
pub mod banner;
pub mod bootstrap;
pub mod cleanup_run;
pub mod cli;
pub mod config;
pub mod device_smoke;
pub mod doctor;
pub mod evidence;
pub mod lock;
pub mod parity;
pub mod publish;
pub mod relay_smoke;
pub mod report;
pub mod run;
pub mod safety;
pub mod scenario_list;
pub mod web_tools;

pub use config::{Config, ConfigError, RunConfig};
pub mod desktop;
pub mod stack;

/// Deterministic fingerprints of complete client bundles.
pub mod fingerprint;

/// Source and build provenance for captured desktop artifacts.
pub mod provenance;

/// Protected desktop identity and action recovery.
pub mod recovery_store;
