//! Mortsom coordinator: configuration, runner, budgets, timeouts, cleanup,
//! reporting and the `mortsom` command-line interface.

pub mod config;

pub use config::{Config, ConfigError, RunConfig};
