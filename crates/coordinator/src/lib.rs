//! Mortsom coordinator: configuration, runner, budgets, timeouts, cleanup,
//! reporting and the `mortsom` command-line interface.

pub mod artifacts;
pub mod budget;
pub mod cli;
pub mod config;
pub mod doctor;
pub mod relay_smoke;
pub mod safety;

pub use config::{Config, ConfigError, RunConfig};
