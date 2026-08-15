//! `mortsom` command-line interface.

use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;

use clap::{Parser, Subcommand};

use mortsom_common::logging::init_logging;

use mortsom_environment::docker::{ContainerRuntime, DockerCli};
use mortsom_environment::relay::NostrProbe;

use mortsom_android_driver::process::TokioRunner;
use mortsom_android_driver::ui::AppiumDriver;

use mortsom_adapter_mobile_legacy::MobileLegacyAdapter;
use mortsom_environment::relay::RelayHandle;

use crate::app_setup::{AppSetupOptions, SetupDeps, run_app_setup};
use crate::config::Config;
use crate::device_smoke::{DeviceSmokeOptions, run_device_smoke};
use crate::doctor::{DoctorOptions, HostProbe, RealHost, RealNet, run_doctor};
use crate::relay_smoke::{RelayObserverSource, SmokeOptions, run_relay_smoke};
use crate::run::{LiveObserver, RunDeps, RunOptions, run_scenarios};

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
    /// List registered scenarios with tags, requirements and value flag.
    List,
    /// Remove what a crashed or interrupted run left on this host (its relay
    /// container, Mortsom's emulators) and report orders it left open.
    Cleanup {
        /// Run id (the artifacts directory name).
        run: String,
    },
    /// Run scenarios end to end (relay, devices, apps, observers, cleanup).
    Run {
        /// Scenario names.
        scenarios: Vec<String>,
        /// Run every scenario carrying this tag.
        #[arg(long)]
        tag: Option<String>,
    },
    /// Application-level operations through the UI.
    App {
        /// App subcommand.
        #[command(subcommand)]
        command: AppCommand,
    },
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

/// App subcommands.
#[derive(Debug, Subcommand)]
pub enum AppCommand {
    /// Start the relay and two devices, onboard both apps through the UI,
    /// configure the daemon, relay and NWC wallets, prove identities differ
    /// and survive a restart, then tear everything down. Moves no value.
    Setup,
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
        Command::List => list_scenarios(),
        Command::Cleanup { run } => cleanup_command(&cli.config, &run).await,
        Command::Run { scenarios, tag } => run_command(&cli.config, scenarios, tag).await,
        Command::App { command } => run_app(&cli.config, command).await,
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

async fn cleanup_command(config_path: &std::path::Path, run_id: &str) -> ExitCode {
    let config = match Config::load(config_path) {
        Ok(config) => config,
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::from(2);
        }
    };
    let env = |name: &str| std::env::var(name).ok();
    let deps = crate::cleanup_run::CleanupDeps {
        runner: &TokioRunner,
        docker: &DockerCli::default(),
        env: &env,
    };
    let summary = crate::cleanup_run::cleanup_run(&config, run_id, &deps).await;
    print!("{}", summary.render());
    if summary.clean {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(1)
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

async fn run_app(config_path: &std::path::Path, command: AppCommand) -> ExitCode {
    let config = match Config::load(config_path) {
        Ok(config) => config,
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::from(2);
        }
    };
    match command {
        AppCommand::Setup => {
            let (Some(appium), Some(flutter)) =
                (RealHost.which("appium"), RealHost.which("flutter"))
            else {
                eprintln!("appium and flutter must be on PATH; run `mortsom doctor`");
                return ExitCode::from(2);
            };
            let ui = AppiumDriver::new(appium);
            let docker = DockerCli::default();
            // The build seeds the relay URL for the configured base port; a
            // run whose relay lands on another port adds it through the UI.
            let seed = RelayHandle {
                container_name: String::new(),
                host_port: config.relay.host_port,
                data_dir: std::path::PathBuf::new(),
                image: String::new(),
            }
            .emulator_url();
            let adapter = MobileLegacyAdapter::new(config.mobile.package.clone(), flutter, seed);
            let env = |name: &str| std::env::var(name).ok();
            let deps = SetupDeps {
                runner: &TokioRunner,
                ui: &ui,
                docker: &docker,
                probe: &NostrProbe,
                adapter: &adapter,
                env: &env,
            };
            match run_app_setup(&config, &AppSetupOptions::default(), &deps).await {
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

fn list_scenarios() -> ExitCode {
    mortsom_scenarios::link();
    if let Err(e) = mortsom_scenario_kit::registry::validate() {
        eprintln!("{e}");
        return ExitCode::from(2);
    }
    for def in mortsom_scenario_kit::registry::all() {
        println!(
            "{:<28} tags=[{}] requires=[{}] moves_value={} timeout={}s",
            def.name,
            def.tags.join(","),
            def.requires
                .iter()
                .map(|c| c.as_str())
                .collect::<Vec<_>>()
                .join(","),
            def.moves_value,
            def.timeout_secs
        );
    }
    ExitCode::SUCCESS
}

async fn run_command(
    config_path: &std::path::Path,
    scenarios: Vec<String>,
    tag: Option<String>,
) -> ExitCode {
    mortsom_scenarios::link();
    let config = match Config::load(config_path) {
        Ok(config) => config,
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::from(2);
        }
    };
    let (Some(appium), Some(flutter)) = (RealHost.which("appium"), RealHost.which("flutter"))
    else {
        eprintln!("appium and flutter must be on PATH; run `mortsom doctor`");
        return ExitCode::from(2);
    };
    let ui = AppiumDriver::new(appium);
    let docker = DockerCli::default();
    let seed = RelayHandle {
        container_name: String::new(),
        host_port: config.relay.host_port,
        data_dir: std::path::PathBuf::new(),
        image: String::new(),
    }
    .emulator_url();
    let adapter: std::sync::Arc<dyn mortsom_app_contract::AppAdapter> = std::sync::Arc::new(
        MobileLegacyAdapter::new(config.mobile.package.clone(), flutter, seed),
    );
    let env = |name: &str| std::env::var(name).ok();
    let wallets =
        |label: &str,
         uri: &mortsom_common::Secret|
         -> Result<std::sync::Arc<dyn mortsom_observers::wallet::WalletReader>, String> {
            mortsom_observers::wallet::NwcReader::new(label, uri, crate::run::WALLET_LOOKUP_TIMEOUT)
                .map(|r| {
                    std::sync::Arc::new(r)
                        as std::sync::Arc<dyn mortsom_observers::wallet::WalletReader>
                })
                .map_err(|e| e.to_string())
        };
    let deps = RunDeps {
        runner: &TokioRunner,
        ui: &ui,
        docker: Arc::new(docker),
        probe: Arc::new(NostrProbe),
        adapter,
        env: &env,
        wallets: &wallets,
        observer: &LiveObserver,
    };
    let options = RunOptions {
        scenarios,
        tag,
        shutdown_grace: None,
    };
    match run_scenarios(&config, &options, &deps).await {
        Ok(summary) => {
            print!("{}", summary.checks.render());
            for result in &summary.results {
                println!(
                    "{:<28} {:?} (attempts {})",
                    result.name, result.outcome, result.attempts
                );
            }
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
