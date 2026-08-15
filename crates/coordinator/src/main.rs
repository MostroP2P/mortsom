//! `mortsom` command-line entry point.

use std::process::ExitCode;

use clap::Parser;
use mortsom_coordinator::cli::{Cli, run};

#[tokio::main]
async fn main() -> ExitCode {
    let cli = Cli::parse();
    run(cli).await
}
