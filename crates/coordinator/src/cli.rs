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
use mortsom_adapter_mobile_v2::MobileV2Adapter;
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
    /// Build the sanitized publication bundle of a run (redacted text
    /// evidence, screenshots by policy, manifest with retry classification).
    Publish {
        /// Run id.
        run: String,
        /// Output directory for the bundle.
        #[arg(long)]
        out: PathBuf,
        /// Screenshot policy: `all` or `none` (pixels cannot be redacted).
        #[arg(long, default_value = "all")]
        screenshots: String,
    },
    /// Cross-app parity: compare two runs of the shared scenarios against
    /// different profiles, per scenario (outcome, state path, relay path,
    /// wallet, cleanup of that scenario's orders, contract); differences
    /// are findings unless listed as intentional.
    Parity {
        /// Run id of the first profile.
        run_a: String,
        /// Run id of the second profile.
        run_b: String,
        /// TOML file of intentional differences (`[[intentional]]` entries).
        #[arg(long)]
        intentional: Option<PathBuf>,
    },
    /// Spend and outcome trend across runs, from their summaries.
    Accounting {
        /// Only runs of this lane (`local`, `ci`, `canary`).
        #[arg(long)]
        lane: Option<String>,
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

/// Whether terminal styling is welcome: a real terminal, no `NO_COLOR`, no
/// dumb terminal. Redirected output stays plain so logs, pipes and CI
/// artifacts read exactly as they always did.
fn use_color() -> bool {
    use std::io::IsTerminal;
    std::io::stdout().is_terminal()
        && std::env::var_os("NO_COLOR").is_none()
        && std::env::var("TERM").is_ok_and(|t| t != "dumb")
}

/// Runs the parsed command and returns the process exit code.
pub async fn run(cli: Cli) -> ExitCode {
    // Stderr immediately; commands that own a run directory attach the
    // redacted `run.log` once the run id exists (`attach_run_log`).
    let _sink = init_logging();
    match cli.command {
        Command::List => list_scenarios(),
        Command::Cleanup { run } => cleanup_command(&cli.config, &run).await,
        Command::Publish {
            run,
            out,
            screenshots,
        } => publish_command(&cli.config, &run, &out, &screenshots),
        Command::Accounting { lane } => accounting_command(&cli.config, lane.as_deref()),
        Command::Parity {
            run_a,
            run_b,
            intentional,
        } => parity_command(&cli.config, &run_a, &run_b, intentional.as_deref()),
        Command::Run { scenarios, tag } => run_command(&cli.config, scenarios, tag).await,
        Command::App { command } => run_app(&cli.config, command).await,
        Command::Device { command } => run_device(&cli.config, command).await,
        Command::Relay { command } => run_relay(&cli.config, command).await,
        Command::Doctor { no_network, json } => {
            let options = DoctorOptions { no_network };
            // The daemon probe waits for events republished on the daemon's
            // own cycle, so it uses the configured health timeout.
            let net = match Config::load(&cli.config) {
                Ok(config) => RealNet::with_daemon_timeout(std::time::Duration::from_secs(
                    config.mostro.health_timeout_secs,
                )),
                Err(_) => RealNet::default(),
            };
            let report = run_doctor(&cli.config, &options, &RealHost, &net).await;
            if json {
                match serde_json::to_string_pretty(&report) {
                    Ok(text) => println!("{text}"),
                    Err(e) => {
                        eprintln!("cannot serialize report: {e}");
                        return ExitCode::from(2);
                    }
                }
            } else {
                print!("{}", report.render_with_color(use_color()));
            }
            if report.is_healthy() {
                ExitCode::SUCCESS
            } else {
                ExitCode::from(1)
            }
        }
    }
}

fn publish_command(
    config_path: &std::path::Path,
    run_id: &str,
    out: &std::path::Path,
    screenshots: &str,
) -> ExitCode {
    let config = match Config::load(config_path) {
        Ok(config) => config,
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::from(2);
        }
    };
    let run_id: mortsom_common::RunId = match run_id.parse() {
        Ok(id) => id,
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::from(2);
        }
    };
    let screenshots = match screenshots.parse() {
        Ok(policy) => policy,
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::from(2);
        }
    };
    let options = crate::publish::PublishOptions { screenshots };
    match crate::publish::publish_run(&config, run_id.as_str(), out, options) {
        Ok(manifest) => {
            print!("{}", manifest.render());
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("{e}");
            ExitCode::from(1)
        }
    }
}

fn parity_command(
    config_path: &std::path::Path,
    run_a: &str,
    run_b: &str,
    intentional: Option<&std::path::Path>,
) -> ExitCode {
    let config = match Config::load(config_path) {
        Ok(config) => config,
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::from(2);
        }
    };
    let ids: Result<(mortsom_common::RunId, mortsom_common::RunId), String> =
        run_a.parse().and_then(|a| run_b.parse().map(|b| (a, b)));
    let (run_a, run_b) = match ids {
        Ok(ids) => ids,
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::from(2);
        }
    };
    let intentional = match intentional.map(crate::parity::load_intentional).transpose() {
        Ok(list) => list.unwrap_or_default(),
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::from(2);
        }
    };
    match crate::parity::compare_runs(
        std::path::Path::new(&config.run.artifacts_dir),
        run_a.as_str(),
        run_b.as_str(),
        &intentional,
    ) {
        Ok(report) => {
            print!("{}", report.render());
            if report.parity {
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

fn accounting_command(config_path: &std::path::Path, lane: Option<&str>) -> ExitCode {
    let config = match Config::load(config_path) {
        Ok(config) => config,
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::from(2);
        }
    };
    let lane = match lane.map(str::parse::<mortsom_common::Lane>).transpose() {
        Ok(lane) => lane,
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::from(2);
        }
    };
    match crate::accounting::build_accounting(std::path::Path::new(&config.run.artifacts_dir), lane)
    {
        Ok(report) => {
            print!("{}", report.render());
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("{e}");
            ExitCode::from(1)
        }
    }
}

/// The adapter for `mobile.profile` (validated by the config loader).
pub fn select_adapter(
    config: &Config,
    flutter: PathBuf,
    relay_seed: String,
) -> Arc<dyn mortsom_app_contract::AppAdapter> {
    let package = config.mobile.package.clone();
    if config.mobile.profile == mortsom_adapter_mobile_v2::PROFILE {
        Arc::new(MobileV2Adapter::new(package, flutter, relay_seed))
    } else {
        Arc::new(MobileLegacyAdapter::new(package, flutter, relay_seed))
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
    let run_id: mortsom_common::RunId = match run_id.parse() {
        Ok(id) => id,
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
    let summary = crate::cleanup_run::cleanup_run(&config, &run_id, &deps).await;
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
                    print!("{}", summary.checks.render_with_color(use_color()));
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
            let (prefix, exact) = match (run, all) {
                (Some(run), false) => {
                    let name = format!("{}-{run}", config.relay.container_name_prefix);
                    (name.clone(), Some(name))
                }
                (None, true) => (format!("{}-", config.relay.container_name_prefix), None),
                _ => {
                    eprintln!("specify --run <id> or --all");
                    return ExitCode::from(2);
                }
            };
            match docker.list_names(&prefix).await {
                Ok(names) => {
                    let mut failed = false;
                    for name in relay_down_targets(names, exact.as_deref()) {
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

/// Containers `relay down` removes out of a prefix listing: with `--run`
/// only the run's exact container name (a prefix match could reach another
/// run whose id extends this one, as `cleanup_run` also guards); with
/// `--all` everything listed.
fn relay_down_targets(names: Vec<String>, exact: Option<&str>) -> Vec<String> {
    match exact {
        Some(exact) => names.into_iter().filter(|n| n == exact).collect(),
        None => names,
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
                    print!("{}", summary.checks.render_with_color(use_color()));
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
            let adapter = select_adapter(&config, flutter, seed);
            let env = |name: &str| std::env::var(name).ok();
            let deps = SetupDeps {
                runner: &TokioRunner,
                ui: &ui,
                docker: &docker,
                probe: &NostrProbe,
                adapter: adapter.as_ref(),
                env: &env,
            };
            match run_app_setup(&config, &AppSetupOptions::default(), &deps).await {
                Ok(summary) => {
                    print!("{}", summary.checks.render_with_color(use_color()));
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
    // One run per host: emulator slots and the canary budget are exclusive.
    let _lock = match crate::lock::RunLock::acquire(std::path::Path::new(&config.run.artifacts_dir))
    {
        Ok(lock) => lock,
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::from(3);
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
    let adapter = select_adapter(&config, flutter, seed);
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
            print!("{}", summary.checks.render_with_color(use_color()));
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

#[cfg(test)]
mod tests {
    use super::relay_down_targets;

    fn names() -> Vec<String> {
        ["relay-run1", "relay-run10", "relay-run2"]
            .map(str::to_owned)
            .to_vec()
    }

    #[test]
    fn relay_down_with_run_removes_only_the_exact_container() {
        assert_eq!(
            relay_down_targets(names(), Some("relay-run1")),
            vec!["relay-run1".to_owned()]
        );
    }

    #[test]
    fn relay_down_with_all_removes_every_listed_container() {
        assert_eq!(relay_down_targets(names(), None), names());
    }
}
