//! Mortsom coordinator: configuration, runner, budgets, timeouts, cleanup,
//! reporting and the `mortsom` command-line interface.

pub mod accounting;
pub mod app_source;
pub mod artifacts;
pub mod banner;
pub mod bootstrap;
pub mod cleanup_run;
pub mod cli;
pub mod config;
pub mod doctor;
pub mod evidence;
pub mod git_source;
pub mod lock;
pub mod mostro_source;
pub mod parity;
pub mod publish;
pub mod relay_smoke;
pub mod report;
pub mod run;
pub mod safety;
pub mod scenario_list;
pub mod style;
pub mod web_tools;

pub use config::{Config, ConfigError, RunConfig};
pub mod desktop;
pub mod stack;

/// Deterministic fingerprints of complete client bundles.
pub mod fingerprint;

/// Source and build provenance for captured desktop artifacts.
pub mod provenance;

/// Managed-source app bundles kept and reused across invocations.
pub mod build_cache;

/// Protected desktop identity and action recovery.
pub mod recovery_store;

/// Terminal recovery outcomes and recorded environment facts.
pub mod recovery_outcome;

/// Terminal abandonment of recovery whose regtest stack was destroyed.
pub mod recovery_abandon;
/// Explicit regtest continuation that preserves ambiguous legacy recovery.
pub mod recovery_isolation;
