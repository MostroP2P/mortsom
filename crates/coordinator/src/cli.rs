//! `mortsom` command-line interface.

use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand};

use mortsom_common::logging::init_logging;

use crate::doctor::{DoctorOptions, RealHost, RealNet, run_doctor};

/// Default configuration file.
pub const DEFAULT_CONFIG: &str = "mortsom.toml";

/// End-to-end test harness for the Mostro mobile application.
#[derive(Debug, Parser)]
#[command(name = "mortsom", version, about)]
pub struct Cli {
    /// Configuration file (a `<name>.local.toml` overlay is merged if present).
    #[arg(long, global = true, default_value = DEFAULT_CONFIG)]
    pub config: PathBuf,
    /// Subcommand.
    #[command(subcommand)]
    pub command: Command,
}

/// Top-level commands.
#[derive(Debug, Subcommand)]
pub enum Command {
    /// Check prerequisites, secrets, daemon, relay port and wallets. Performs
    /// no payment and creates no order.
    Doctor {
        /// Skip every check that opens a network connection.
        #[arg(long)]
        no_network: bool,
        /// Print the report as JSON instead of text.
        #[arg(long)]
        json: bool,
    },
}

/// Runs the parsed command and returns the process exit code.
pub async fn run(cli: Cli) -> ExitCode {
    // Stderr immediately; commands that own a run directory attach the
    // redacted `run.log` once the run id exists (`attach_run_log`).
    let _sink = init_logging();
    match cli.command {
        Command::Doctor { no_network, json } => {
            let options = DoctorOptions { no_network };
            let report = run_doctor(&cli.config, &options, &RealHost, &RealNet).await;
            if json {
                match serde_json::to_string_pretty(&report) {
                    Ok(text) => println!("{text}"),
                    Err(e) => {
                        eprintln!("cannot serialize report: {e}");
                        return ExitCode::from(2);
                    }
                }
            } else {
                print!("{}", report.render());
            }
            if report.is_healthy() {
                ExitCode::SUCCESS
            } else {
                ExitCode::from(1)
            }
        }
    }
}
