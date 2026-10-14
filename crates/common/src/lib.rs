//! Mortsom foundation: secrets and redaction, `env:` resolution, run
//! identifiers, error taxonomy, clock and logging shared by every crate.
//!
//! This crate holds the pieces every other Mortsom crate needs without
//! creating a dependency cycle through the coordinator. It contains no I/O
//! beyond what the logging initializer requires.

pub mod actor;
pub mod check;
pub mod env_resolve;
pub mod failure;
pub mod lane;
pub mod logging;
pub mod redact;
pub mod run_id;
pub mod secret;
pub mod terminal;
pub mod tls;

pub use actor::Actor;
pub use check::{CheckResult, CheckStatus, DoctorReport};
pub use env_resolve::{MissingEnv, resolve_env_with};
pub use failure::FailureCategory;
pub use lane::Lane;
pub use redact::Redactor;
pub use run_id::RunId;
pub use secret::Secret;
