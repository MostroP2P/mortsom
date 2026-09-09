//! `mortsom` command-line interface.

mod maintenance;
mod recover;
mod recover_audit;
mod run;
use maintenance::{accounting_command, cleanup_command, parity_command, publish_command};
use run::run_command;

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
pub const DEFAULT_CONFIG: &str = "config/desktop.example.toml";

/// End-to-end test harness for the Mostro mobile application.
#[derive(Debug, Parser)]
#[command(name = "mortsom", version, about)]
pub struct Cli {
    /// Configuration file (a `<name>.local.toml` overlay is merged if present).
    #[arg(long, global = true, default_value = DEFAULT_CONFIG)]
    pub config: PathBuf,
    /// Ortsom checkout owning the persistent regtest stack.
    #[arg(long, global = true, default_value = "../ortsom")]
    pub ortsom_dir: PathBuf,
    /// Subcommand.
    #[command(subcommand)]
    pub command: Command,
}

/// Top-level commands.
#[derive(Debug, Subcommand)]
pub enum Command {
    /// Manage the persistent regtest environment through the Ortsom CLI.
    Stack {
        /// Lifecycle action.
        #[command(subcommand)]
        action: StackAction,
    },
    /// List registered scenarios with tags, requirements and value flag.
    List,
    /// Recover retained desktop identities and orders, or clean legacy Android resources.
    /// Refuses unresolved ownership and changed stack identities.
    Cleanup {
        /// Run id (the artifacts directory name).
        run: String,
        /// Name the order of an actor's unresolved submission, as
        /// `actor=order-id`, when the app never listed it. The order must
        /// still be shown closed by the relay or closed through the UI.
        #[arg(long = "attribute-order", value_name = "ACTOR=ORDER_ID")]
        attribute_order: Vec<String>,
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
        /// Client target for Alice and Bob.
        #[arg(long, default_value = "linux")]
        platform: mortsom_desktop_driver::Platform,
        /// Prebuilt native executable or Web bundle; otherwise builds the source.
        #[arg(long)]
        app_artifact: Option<PathBuf>,
        /// Xvfb executable for isolated Linux desktops.
        #[arg(long, default_value = "Xvfb")]
        xvfb: PathBuf,
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
        /// Client target whose prerequisites are checked.
        #[arg(long, default_value = "linux")]
        platform: mortsom_desktop_driver::Platform,
        /// Validate an existing artifact instead of source/build prerequisites.
        #[arg(long)]
        app_artifact: Option<PathBuf>,
        /// Skip every check that opens a network connection.
        #[arg(long)]
        no_network: bool,
        /// Print the report as JSON instead of text.
        #[arg(long)]
        json: bool,
    },
}

/// Persistent stack lifecycle actions.
#[derive(Debug, Subcommand)]
pub enum StackAction {
    /// Pull the pinned mostrod release and prepare the regtest environment.
    Up {
        /// Daemon image to install instead of the pinned release, e.g. a
        /// build from a mostro branch. Recorded in the stack fingerprint.
        #[arg(long)]
        mostro_image: Option<String>,
    },
    /// Inspect the persistent services.
    Status,
    /// Destroy the regtest stack, including generated credentials and state.
    Down,
}

async fn stack_cli(checkout: &std::path::Path, action: StackAction) -> ExitCode {
    let (action, image) = match action {
        StackAction::Up { mostro_image } => ("up", mostro_image),
        StackAction::Status => ("status", None),
        StackAction::Down => ("down", None),
    };
    let image = image.unwrap_or_else(|| crate::stack::DEFAULT_MOSTRO_IMAGE.to_owned());
    let binary = crate::stack::binary(checkout);
    match crate::stack::invoke_with_image(&TokioRunner, checkout, &binary, action, &image).await {
        Ok(output) => {
            print!("{output}");
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("{error}");
            ExitCode::from(2)
        }
    }
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

fn lock_mutation(
    command: &Command,
    host: &dyn HostProbe,
) -> Result<Option<crate::lock::RunLock>, String> {
    let mutates = matches!(
        command,
        Command::Run { .. }
            | Command::Cleanup { .. }
            | Command::App { .. }
            | Command::Device { .. }
            | Command::Relay { .. }
            | Command::Stack {
                action: StackAction::Up { .. } | StackAction::Down
            }
    );
    if !mutates {
        return Ok(None);
    }
    let home = host
        .env("HOME")
        .ok_or("HOME is required for the operator's exclusive mutation lock")?;
    crate::lock::RunLock::acquire_host(std::path::Path::new(&home))
        .map(Some)
        .map_err(|error| error.to_string())
}

/// Runs the parsed command and returns the process exit code.
pub async fn run(cli: Cli) -> ExitCode {
    // Stderr immediately; commands that own a run directory attach the
    // redacted `run.log` once the run id exists (`attach_run_log`).
    let _sink = init_logging();
    let _host_lock = match lock_mutation(&cli.command, &RealHost) {
        Ok(lock) => lock,
        Err(error) => {
            eprintln!("{error}");
            return ExitCode::from(3);
        }
    };
    match cli.command {
        Command::Stack { action } => stack_cli(&cli.ortsom_dir, action).await,
        Command::List => list_scenarios(),
        Command::Cleanup {
            run,
            attribute_order,
        } => cleanup_command(&cli.config, &cli.ortsom_dir, &run, &attribute_order).await,
        Command::Publish {
            run,
            out,
            screenshots,
        } => publish_command(&cli.config, &cli.ortsom_dir, &run, &out, &screenshots),
        Command::Accounting { lane } => accounting_command(&cli.config, lane.as_deref()),
        Command::Parity {
            run_a,
            run_b,
            intentional,
        } => parity_command(&cli.config, &run_a, &run_b, intentional.as_deref()),
        Command::Run {
            scenarios,
            tag,
            platform,
            app_artifact,
            xvfb,
        } => {
            run_command(
                &cli.config,
                &cli.ortsom_dir,
                scenarios,
                tag,
                platform,
                app_artifact,
                xvfb,
            )
            .await
        }
        Command::App { command } => run_app(&cli.config, command).await,
        Command::Device { command } => run_device(&cli.config, command).await,
        Command::Relay { command } => run_relay(&cli.config, command).await,
        Command::Doctor {
            no_network,
            json,
            platform,
            app_artifact,
        } => {
            let options = DoctorOptions { no_network };
            // The daemon probe waits for events republished on the daemon's
            // own cycle, so it uses the configured health timeout.
            let net = match Config::diagnose(&cli.config, |name| RealHost.env(name)).config {
                Some(config) => RealNet::with_daemon_timeout(std::time::Duration::from_secs(
                    config.mostro.health_timeout_secs,
                )),
                None => RealNet::default(),
            };
            let report = if platform == mortsom_desktop_driver::Platform::Android {
                run_doctor(&cli.config, &options, &RealHost, &net).await
            } else {
                crate::doctor::run_desktop_doctor(
                    &cli.config,
                    &cli.ortsom_dir,
                    &crate::doctor::DesktopDoctorOptions {
                        platform,
                        artifact: app_artifact,
                        no_network,
                    },
                    &RealHost,
                    &net,
                    &TokioRunner,
                )
                .await
            };
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
            // Only the build is used here, but the adapter needs the relay
            // seed it bakes into the APK; the device smoke opens no order,
            // so the port it names is the configured one.
            let seed = RelayHandle {
                container_name: String::new(),
                host_port: config.relay.host_port,
                data_dir: std::path::PathBuf::new(),
                image: String::new(),
            }
            .emulator_url();
            let flutter = RealHost.which("flutter").unwrap_or_default();
            let adapter = select_adapter(&config, flutter, seed);
            match run_device_smoke(
                &config,
                &DeviceSmokeOptions::default(),
                &TokioRunner,
                &ui,
                adapter.as_ref(),
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

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::relay_down_targets;

    #[test]
    fn run_defaults_to_linux_and_accepts_only_the_three_named_platforms() {
        use clap::Parser;
        use mortsom_desktop_driver::Platform;
        let cli = super::Cli::try_parse_from(["mortsom", "run", "happy_sell"]).unwrap();
        assert!(matches!(
            cli.command,
            super::Command::Run {
                platform: Platform::Linux,
                ..
            }
        ));
        assert_eq!(
            cli.config,
            std::path::Path::new("config/desktop.example.toml")
        );
        for (name, expected) in [
            ("linux", Platform::Linux),
            ("web", Platform::Web),
            ("android", Platform::Android),
        ] {
            let cli = super::Cli::try_parse_from([
                "mortsom",
                "--config",
                "explicit.local.toml",
                "run",
                "happy_sell",
                "--platform",
                name,
            ])
            .unwrap();
            assert!(
                matches!(cli.command, super::Command::Run { platform, .. } if platform == expected)
            );
            assert_eq!(cli.config, std::path::Path::new("explicit.local.toml"));
        }
        assert!(super::Cli::try_parse_from(["mortsom", "run", "--platform", "unknown"]).is_err());
    }

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
