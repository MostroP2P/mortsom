//! Mortsom's driver for [mostro-cli](https://github.com/MostroP2P/mostro-cli),
//! the `cli` client profile: each actor action is one child process speaking
//! the CLI's machine interface (`--output json`, contract version 1).
//!
//! The CLI is always a subprocess, never a linked library. Each actor owns a
//! private `MCLI_DIR` and `HOME`; the child inherits no other environment;
//! invoices travel on standard input, never in the process arguments; and
//! every invocation is recorded in a sanitized transcript that serves as the
//! actor's evidence in place of screenshots.

pub mod call;
pub mod envelope;
pub mod error;
pub mod process;
pub mod transcript;

pub use call::{Call, NodeTarget};
pub use envelope::{Envelope, Status, unsigned_of};
pub use error::{CliError, category_of_code};
pub use process::{
    CliProcess, ProcessFailed, ProcessFailure, ProcessOutput, ProcessRequest, TokioCliProcess,
};
