//! Mortsom coordinator: configuration, runner, budgets, timeouts, cleanup,
//! reporting and the `mortsom` command-line interface.

pub mod accounting;
pub mod app_setup;
pub mod artifacts;
pub mod bootstrap;
pub mod cleanup_run;
pub mod cli;
pub mod config;
pub mod device_smoke;
pub mod doctor;
pub mod evidence;
pub mod lock;
pub mod publish;
pub mod relay_smoke;
pub mod report;
pub mod run;
pub mod safety;

pub use config::{Config, ConfigError, RunConfig};
