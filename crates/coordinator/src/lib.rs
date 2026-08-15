//! Mortsom coordinator: configuration, runner, budgets, timeouts, cleanup,
//! reporting and the `mortsom` command-line interface.

pub mod cli;
pub mod config;
pub mod doctor;

pub use config::{Config, ConfigError, RunConfig};
