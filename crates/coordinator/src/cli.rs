//! `mortsom` command-line interface.

use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand};

use mortsom_common::logging::init_logging;

use mortsom_environment::docker::{ContainerRuntime, DockerCli};
use mortsom_environment::relay::NostrProbe;

use mortsom_android_driver::process::TokioRunner;
use mortsom_android_driver::ui::AppiumDriver;

use crate::config::Config;
use crate::device_smoke::{DeviceSmokeOptions, run_device_smoke};
use crate::doctor::{DoctorOptions, HostProbe, RealHost, RealNet, run_doctor};
use crate::relay_smoke::{RelayObserverSource, SmokeOptions, run_relay_smoke};

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
    /// Emulator farm operations.
    Device {
        /// Device subcommand.
        #[command(subcommand)]
        command: DeviceCommand,
    },
    /// Run-owned relay operations.
    Relay {
        /// Relay subcommand.
        #[command(subcommand)]
        command: RelayCommand,
    },
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

/// Device subcommands.
#[derive(Debug, Subcommand)]
pub enum DeviceCommand {
    /// Bring two clean devices up, install the configured APK on both,
    /// launch it, record the initial screens and tear everything down.
    Smoke,
}

/// Relay subcommands.
#[derive(Debug, Subcommand)]
pub enum RelayCommand {
    /// Start a relay, prove the local daemon uses it, collect evidence and
    /// tear it down. No app, no order, no payment.
    Smoke {
        /// Keep the relay container and data when the smoke fails.
        #[arg(long)]
        keep_on_failure: bool,
    },
    /// Remove relay containers left behind by a run. Scoped to one run id
    /// unless `--all` is given, so a parallel run is never touched by mistake.
    Down {
        /// Run id whose relay container should be removed.
        #[arg(long, conflicts_with = "all")]
        run: Option<String>,
        /// Remove every Mortsom relay container on this host.
        #[arg(long)]
        all: bool,
    },
}

/// Runs the parsed command and returns the process exit code.
pub async fn run(cli: Cli) -> ExitCode {
    // Stderr immediately; commands that own a run directory attach the
    // redacted `run.log` once the run id exists (`attach_run_log`).
    let _sink = init_logging();
    match cli.command {
        Command::Device { command } => run_device(&cli.config, command).await,
        Command::Relay { command } => run_relay(&cli.config, command).await,
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

async fn run_relay(config_path: &std::path::Path, command: RelayCommand) -> ExitCode {
    let config = match Config::load(config_path) {
        Ok(config) => config,
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::from(2);
        }
    };
    let docker = DockerCli::default();
    match command {
        RelayCommand::Smoke { keep_on_failure } => {
            let options = SmokeOptions { keep_on_failure };
            match run_relay_smoke(
                &config,
                &options,
                &docker,
                &NostrProbe,
                &RelayObserverSource,
            )
            .await
            {
                Ok(summary) => {
                    print!("{}", summary.checks.render());
                    println!("artifacts: {}/{}", config.run.artifacts_dir, summary.run_id);
                    if summary.passed {
                        ExitCode::SUCCESS
                    } else {
                        ExitCode::from(1)
                    }
                }
                Err(e) => {
                    eprintln!("{e}");
                    ExitCode::from(2)
                }
            }
        }
        RelayCommand::Down { run, all } => {
            let prefix = match (run, all) {
                (Some(run), false) => format!("{}-{run}", config.relay.container_name_prefix),
                (None, true) => format!("{}-", config.relay.container_name_prefix),
                _ => {
                    eprintln!("specify --run <id> or --all");
                    return ExitCode::from(2);
                }
            };
            match docker.list_names(&prefix).await {
                Ok(names) => {
                    let mut failed = false;
                    for name in names {
                        match docker.stop_and_remove(&name).await {
                            Ok(()) => println!("removed {name}"),
                            Err(e) => {
                                failed = true;
                                eprintln!("cannot remove {name}: {e}");
                            }
                        }
                    }
                    if failed {
                        ExitCode::from(1)
                    } else {
                        ExitCode::SUCCESS
                    }
                }
                Err(e) => {
                    eprintln!("{e}");
                    ExitCode::from(2)
                }
            }
        }
    }
}

async fn run_device(config_path: &std::path::Path, command: DeviceCommand) -> ExitCode {
    let config = match Config::load(config_path) {
        Ok(config) => config,
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::from(2);
        }
    };
    match command {
        DeviceCommand::Smoke => {
            let Some(appium) = RealHost.which("appium") else {
                eprintln!("appium not found on PATH; run `mortsom doctor`");
                return ExitCode::from(2);
            };
            let ui = AppiumDriver::new(appium);
            let env = |name: &str| std::env::var(name).ok();
            match run_device_smoke(
                &config,
                &DeviceSmokeOptions::default(),
                &TokioRunner,
                &ui,
                &env,
            )
            .await
            {
                Ok(summary) => {
                    print!("{}", summary.checks.render());
                    println!("artifacts: {}/{}", config.run.artifacts_dir, summary.run_id);
                    if summary.passed {
                        ExitCode::SUCCESS
                    } else {
                        ExitCode::from(1)
                    }
                }
                Err(e) => {
                    eprintln!("{e}");
                    ExitCode::from(2)
                }
            }
        }
    }
}
