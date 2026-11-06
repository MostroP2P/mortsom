//! `mortsom` command-line interface.

mod abandon;
mod isolation;
mod maintenance;
mod prepared;
mod recover;
pub(crate) mod recover_audit;
mod run;
mod run_report;
mod source;
use maintenance::{accounting_command, cleanup_command, parity_command, publish_command};
use run::run_command;
pub use source::SourceOverrides;

use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;

use clap::{Parser, Subcommand};

use mortsom_common::logging::init_logging;

use mortsom_environment::docker::{ContainerRuntime, DockerCli};
use mortsom_environment::relay::NostrProbe;

use mortsom_ui_driver::process::TokioRunner;

use mortsom_adapter_mobile_v2::MobileV2Adapter;

use crate::config::Config;
use crate::doctor::{HostProbe, RealHost, RealNet};
use crate::relay_smoke::{RelayObserverSource, SmokeOptions, run_relay_smoke};
use crate::run::{LiveObserver, RunDeps, RunOptions, run_scenarios};

/// Default configuration file.
pub const DEFAULT_CONFIG: &str = "config/desktop.example.toml";

/// End-to-end test harness for the Mostro application on Linux and Web.
#[derive(Debug, Parser)]
#[command(name = "mortsom", version, about)]
pub struct Cli {
    /// Configuration file (a `<name>.local.toml` overlay is merged if present).
    #[arg(long, global = true, default_value = DEFAULT_CONFIG)]
    pub config: PathBuf,
    /// Directory holding the stack's compose file and its generated state.
    /// The host ports allow one stack per machine: point every checkout at
    /// the same directory to share it.
    #[arg(
        long,
        global = true,
        env = "MORTSOM_STACK_DIR",
        default_value = "stack"
    )]
    pub stack_dir: PathBuf,
    /// Subcommand.
    #[command(subcommand)]
    pub command: Command,
}

/// Top-level commands.
#[derive(Debug, Subcommand)]
pub enum Command {
    /// Create, inspect or destroy the regtest environment.
    Stack {
        /// Lifecycle action.
        #[command(subcommand)]
        action: StackAction,
    },
    /// List registered scenarios by family: which move sats, which need a
    /// dispute solver, and the tags `--tag` selects.
    List {
        /// Also show the capabilities an app profile must support and the
        /// deadline that aborts a stuck attempt.
        #[arg(long)]
        details: bool,
    },
    /// Recover retained desktop identities and orders of an interrupted run.
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
    /// Close a recovery journal whose regtest stack no longer exists.
    Recovery {
        /// Recovery action.
        #[command(subcommand)]
        action: RecoveryAction,
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
    /// Run scenarios end to end (stack relay, clients, observers, cleanup):
    /// every registered scenario unless names or `--tag` narrow it.
    Run {
        /// Keep this exact legacy journal pending while using a different,
        /// verified Mortsom regtest stack. Repeat for every pending legacy run.
        #[arg(long, value_name = "RUN_ID")]
        preserve_legacy_recovery: Vec<mortsom_common::RunId>,
        /// App platform for Alice and Bob (`linux` by default). Not with
        /// profile `cli`, which drives mostro-cli.
        #[arg(long)]
        platform: Option<mortsom_desktop_driver::Platform>,
        /// Client profile, overriding `mobile.profile`: `mobile-v2` (the
        /// app) or `cli` (mostro-cli).
        #[arg(long, value_name = "PROFILE")]
        profile: Option<String>,
        /// The mostro-cli to drive with profile `cli`.
        #[command(flatten)]
        cli: crate::mostro_cli_build::CliSelection,
        /// Prebuilt native executable or Web bundle; otherwise builds the source.
        #[arg(long)]
        app_artifact: Option<PathBuf>,
        /// Build the fetched app even when an identical kept bundle exists,
        /// and replace it.
        #[arg(long, conflicts_with = "app_artifact")]
        rebuild: bool,
        /// Override configured source selection.
        #[command(flatten)]
        source: SourceOverrides,
        /// Xvfb executable for isolated Linux desktops.
        #[arg(long, default_value = "Xvfb")]
        xvfb: PathBuf,
        /// Scenario names. With neither names nor `--tag`, every scenario
        /// runs, each as its own run with fresh actors.
        scenarios: Vec<String>,
        /// Run every scenario carrying this tag (a family, or `smoke`). Each
        /// scenario is its own run with fresh actors.
        #[arg(long)]
        tag: Option<String>,
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
        /// App platform whose prerequisites are checked (`linux` by
        /// default). Not with profile `cli`.
        #[arg(long)]
        platform: Option<mortsom_desktop_driver::Platform>,
        /// Client profile, overriding `mobile.profile`: `mobile-v2` or `cli`.
        #[arg(long, value_name = "PROFILE")]
        profile: Option<String>,
        /// The mostro-cli to check with profile `cli`.
        #[command(flatten)]
        cli: crate::mostro_cli_build::CliSelection,
        /// Validate an existing artifact instead of source/build prerequisites.
        #[arg(long)]
        app_artifact: Option<PathBuf>,
        /// Override configured source selection.
        #[command(flatten)]
        source: SourceOverrides,
        /// Skip every check that opens a network connection.
        #[arg(long)]
        no_network: bool,
        /// Print the report as JSON instead of text.
        #[arg(long)]
        json: bool,
    },
}

/// Recovery journal actions other than `cleanup`.
#[derive(Debug, Subcommand)]
pub enum RecoveryAction {
    /// Close an unresolved journal as abandoned, only when its Mortsom-owned
    /// regtest stack is provably destroyed. Evidence is preserved; the run
    /// stays failed and is never reported as cleaned up.
    Abandon {
        /// Run id of the journal.
        run: String,
        /// Why the journal is abandoned.
        #[arg(long, value_enum)]
        reason: crate::recovery_store::AbandonReason,
        /// The same run id again, as confirmation.
        #[arg(long = "confirm-run", value_name = "RUN_ID")]
        confirm_run: String,
    },
}

/// Persistent stack lifecycle actions.
#[derive(Debug, Subcommand)]
pub enum StackAction {
    /// Prepare the regtest environment with mostrod built from Mostro's
    /// `main`, from another branch or pull request, or from a published
    /// image.
    ///
    /// With no option the daemon is Mostro's `main` at its current head,
    /// built here. `--mostro-ref` builds a branch or pull request instead,
    /// and `--mostro-image` runs a daemon image published elsewhere.
    Up {
        /// Published daemon image to pull instead of building Mostro's
        /// `main`. Recorded in the stack fingerprint.
        #[arg(long, value_name = "IMAGE", conflicts_with = "mostro_ref")]
        mostro_image: Option<String>,
        /// Mostro branch or https://github.com/MostroP2P/mostro/pull/123 to
        /// build instead of `main`, at its exact head commit. Fetched on
        /// every run; a failed fetch or build never falls back to an older
        /// image.
        #[arg(long, value_name = "BRANCH_OR_PR_URL")]
        mostro_ref: Option<crate::mostro_source::Selector>,
        /// Remove a `mortsom-regtest` stack this directory does not own --
        /// another checkout's, or resources left without containers -- and
        /// build here instead. The owner's generated state goes stale.
        #[arg(long)]
        replace: bool,
        /// Start the daemon with its anti-abuse bond switched on, from
        /// `stack/mostro-bond.tpl.toml`: every taker locks a bond before the
        /// trade starts. The `bond` family needs it; no other family is
        /// written for a bonded daemon.
        #[arg(long)]
        bond: bool,
    },
    /// Inspect the persistent services.
    Status,
    /// Show a service's log (mostrod by default), by service name.
    Logs {
        /// `mostro`, `relay`, `bitcoind`, `lnd-alice`, `lnd-bob` or `lnd-carol`.
        #[arg(default_value = "mostro")]
        service: String,
        /// Keep streaming until interrupted.
        #[arg(short, long)]
        follow: bool,
        /// Only entries newer than this: a duration (`10m`, `2h`) or a timestamp.
        #[arg(long)]
        since: Option<String>,
        /// Only the last N lines.
        #[arg(long)]
        tail: Option<u32>,
    },
    /// Destroy the regtest stack, including generated credentials and state.
    Down,
}

/// Runs a plan with the terminal as its output, for as long as it runs: a
/// followed log ends only when the operator interrupts it.
async fn stream_to_terminal(plan: &mortsom_ui_driver::process::Command) -> ExitCode {
    let status = tokio::process::Command::new(&plan.program)
        .args(&plan.args)
        .envs(&plan.env)
        .status()
        .await;
    match status {
        Ok(status) if status.success() => ExitCode::SUCCESS,
        Ok(status) => ExitCode::from(status.code().unwrap_or(2).clamp(1, 255) as u8),
        Err(error) => {
            eprintln!("cannot run {}: {error}", plan.display());
            ExitCode::from(2)
        }
    }
}

async fn stack_cli(stack_dir: &std::path::Path, action: StackAction) -> ExitCode {
    use crate::stack::{self, Layout};
    let layout = match Layout::new(stack_dir) {
        Ok(layout) => layout,
        Err(error) => {
            eprintln!("{error}");
            return ExitCode::from(2);
        }
    };
    let operation = match &action {
        StackAction::Up { replace, .. } => {
            Some(crate::recovery_abandon::LifecycleOperation::Up { replace: *replace })
        }
        StackAction::Down => Some(crate::recovery_abandon::LifecycleOperation::Down),
        StackAction::Status | StackAction::Logs { .. } => None,
    };
    if let Some(operation) = operation
        && let Err(error) = abandon::guard_stack(operation, layout.root()).await
    {
        eprintln!("{error}");
        return ExitCode::from(2);
    }
    let outcome = match action {
        StackAction::Up {
            mostro_image,
            mostro_ref,
            replace,
            bond,
        } => {
            let daemon = stack::DaemonSource::from_options(mostro_image, mostro_ref);
            let options = stack::UpOptions {
                daemon,
                source_cache: crate::mostro_source::WORK_DIR.into(),
                timing: stack::Timing::default(),
                replace,
                bond,
            };
            stack::up(&TokioRunner, &stack::LiveRelay, &layout, &options)
                .await
                .map(|descriptor| {
                    format!(
                        "stack ready\ndaemon      {}\nimage       {}\nimage id    {}\nsource      {}\nbond        {}\n",
                        descriptor.daemon_pubkey,
                        descriptor.daemon_image.reference,
                        descriptor.daemon_image.image_id,
                        descriptor.daemon_source,
                        crate::stack::describe_bond(descriptor.declared_limits.bond.as_ref())
                    )
                })
        }
        StackAction::Status => match stack::status(&TokioRunner, &layout).await {
            Ok(status) => {
                print!("{}", status.render_with_color(use_color()));
                // Non-zero until usable, so the command can be waited on.
                return if status.is_ready() {
                    ExitCode::SUCCESS
                } else {
                    ExitCode::from(1)
                };
            }
            Err(error) => Err(error),
        },
        StackAction::Logs {
            service,
            follow,
            since,
            tail,
        } => {
            let options = stack::LogsOptions {
                service,
                follow,
                since,
                tail,
            };
            match stack::logs(&TokioRunner, &layout, &options).await {
                Ok(plan) => return stream_to_terminal(&plan).await,
                Err(error) => Err(error),
            }
        }
        StackAction::Down => stack::down(&TokioRunner, &layout)
            .await
            .map(|()| "stack removed\n".to_owned()),
    };
    match outcome {
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

/// Whether stdout welcomes colour (see [`mortsom_common::terminal`]):
/// redirected output stays plain unless the operator forces it.
fn use_color() -> bool {
    mortsom_common::terminal::color_on(mortsom_common::terminal::Stream::Stdout)
}

/// The same question for the stream the banner and the logs go to.
fn use_color_on_stderr() -> bool {
    mortsom_common::terminal::color_on(mortsom_common::terminal::Stream::Stderr)
}

fn lock_mutation(
    command: &Command,
    host: &dyn HostProbe,
) -> Result<Option<crate::lock::RunLock>, String> {
    let mutates = matches!(
        command,
        Command::Run { .. }
            | Command::Cleanup { .. }
            | Command::Recovery { .. }
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
    // Before anything can fail, and on stderr so `doctor --json` and `list`
    // keep their pipes clean.
    eprint!(
        "{}",
        crate::banner::render(
            &crate::banner::caption(&cli.command, &cli.config, &cli.stack_dir),
            crate::style::Style::detect(mortsom_common::terminal::Stream::Stderr)
        )
    );
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
        Command::Stack { action } => stack_cli(&cli.stack_dir, action).await,
        Command::List { details } => list_scenarios(details),
        Command::Cleanup {
            run,
            attribute_order,
        } => cleanup_command(&cli.config, &cli.stack_dir, &run, &attribute_order).await,
        Command::Recovery {
            action:
                RecoveryAction::Abandon {
                    run,
                    reason,
                    confirm_run,
                },
        } => abandon::abandon_command(&cli.stack_dir, &run, reason, &confirm_run).await,
        Command::Publish {
            run,
            out,
            screenshots,
        } => publish_command(&cli.config, &cli.stack_dir, &run, &out, &screenshots),
        Command::Accounting { lane } => accounting_command(&cli.config, lane.as_deref()),
        Command::Parity {
            run_a,
            run_b,
            intentional,
        } => parity_command(&cli.config, &run_a, &run_b, intentional.as_deref()),
        Command::Run {
            preserve_legacy_recovery,
            scenarios,
            tag,
            platform,
            profile,
            cli: cli_selection,
            app_artifact,
            rebuild,
            source,
            xvfb,
        } => {
            run_command(
                &cli.config,
                &cli.stack_dir,
                run::RunSelection {
                    preserve_legacy_recovery,
                    scenarios,
                    tag,
                    platform,
                    profile,
                    cli: cli_selection,
                    artifact: app_artifact,
                    policy: if rebuild {
                        crate::build_cache::Policy::Rebuild
                    } else {
                        crate::build_cache::Policy::Reuse
                    },
                    source,
                    xvfb,
                },
            )
            .await
        }
        Command::Relay { command } => run_relay(&cli.config, command).await,
        Command::Doctor {
            no_network,
            json,
            platform,
            profile,
            cli: cli_selection,
            app_artifact,
            source,
        } => {
            // The daemon probe waits for events republished on the daemon's
            // own cycle, so it uses the configured health timeout.
            let net = match Config::diagnose_with_source(
                &cli.config,
                |name| RealHost.env(name),
                source.selection().as_ref(),
            )
            .config
            {
                Some(config) => RealNet::with_daemon_timeout(std::time::Duration::from_secs(
                    config.mostro.health_timeout_secs,
                )),
                None => RealNet::default(),
            };
            let report = crate::doctor::run_desktop_doctor(
                &cli.config,
                &cli.stack_dir,
                &crate::doctor::DesktopDoctorOptions {
                    platform,
                    profile,
                    cli: cli_selection,
                    artifact: app_artifact,
                    source,
                    no_network,
                },
                &RealHost,
                &net,
                &TokioRunner,
            )
            .await;
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

/// The adapter of the app profile, `mobile-v2`.
pub fn select_adapter() -> Arc<dyn mortsom_app_contract::AppAdapter> {
    Arc::new(MobileV2Adapter::new())
}

/// The adapter for the client `desktop` prepared: the CLI's, with what its
/// discovery offers, or the app's.
pub async fn adapter_for(
    desktop: &crate::run::DesktopOptions,
    config: &Config,
) -> Result<Arc<dyn mortsom_app_contract::AppAdapter>, String> {
    match desktop.platform {
        mortsom_desktop_driver::Platform::Cli => {
            crate::mostro_cli::adapter_for(desktop, config).await
        }
        _ => Ok(select_adapter()),
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

fn list_scenarios(details: bool) -> ExitCode {
    mortsom_scenarios::link();
    if let Err(e) = mortsom_scenario_kit::registry::validate() {
        eprintln!("{e}");
        return ExitCode::from(2);
    }
    let defs = mortsom_scenario_kit::registry::all();
    let listing = crate::scenario_list::Listing::from_defs(&defs);
    let detail = if details {
        crate::scenario_list::Detail::Full
    } else {
        crate::scenario_list::Detail::Summary
    };
    print!(
        "{}",
        listing.render(
            crate::style::Style::detect(mortsom_common::terminal::Stream::Stdout),
            detail
        )
    );
    ExitCode::SUCCESS
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::relay_down_targets;

    #[test]
    fn run_defaults_to_linux_and_accepts_only_the_two_named_platforms() {
        use clap::Parser;
        use mortsom_desktop_driver::Platform;
        let cli = super::Cli::try_parse_from(["mortsom", "run", "happy_sell"]).unwrap();
        assert!(matches!(
            cli.command,
            super::Command::Run {
                platform: None,
                profile: None,
                ..
            }
        ));
        assert_eq!(
            cli.config,
            std::path::Path::new("config/desktop.example.toml")
        );
        for (name, expected) in [("linux", Platform::Linux), ("web", Platform::Web)] {
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
                matches!(cli.command, super::Command::Run { platform, .. } if platform == Some(expected))
            );
            assert_eq!(cli.config, std::path::Path::new("explicit.local.toml"));
        }
        assert!(super::Cli::try_parse_from(["mortsom", "run", "--platform", "cli"]).is_err());
        assert!(super::Cli::try_parse_from(["mortsom", "run", "--platform", "unknown"]).is_err());
        assert!(super::Cli::try_parse_from(["mortsom", "run", "--platform", "android"]).is_err());
        assert!(super::Cli::try_parse_from(["mortsom", "device", "smoke"]).is_err());
        assert!(super::Cli::try_parse_from(["mortsom", "app", "setup"]).is_err());
    }

    #[test]
    fn run_without_names_or_a_tag_parses_with_no_selection() {
        use clap::Parser;

        let cli = super::Cli::try_parse_from(["mortsom", "run"]).unwrap();

        assert!(matches!(
            cli.command,
            super::Command::Run { ref scenarios, tag: None, .. } if scenarios.is_empty()
        ));
    }

    #[test]
    fn rebuild_is_off_by_default_and_refused_with_a_prebuilt_artifact() {
        use clap::Parser;
        for (args, expected) in [
            (vec!["mortsom", "run", "happy_sell"], false),
            (vec!["mortsom", "run", "happy_sell", "--rebuild"], true),
        ] {
            let cli = super::Cli::try_parse_from(args).unwrap();
            assert!(matches!(
                cli.command,
                super::Command::Run { rebuild, .. } if rebuild == expected
            ));
        }
        assert!(
            super::Cli::try_parse_from([
                "mortsom",
                "run",
                "--rebuild",
                "--app-artifact",
                "/bundle/mostro"
            ])
            .is_err()
        );
    }

    #[test]
    fn stack_dir_is_read_from_mortsom_stack_dir_when_the_flag_is_absent() {
        use clap::CommandFactory;
        let command = super::Cli::command();
        let stack_dir = command
            .get_arguments()
            .find(|arg| arg.get_id() == "stack_dir")
            .unwrap();
        assert_eq!(
            stack_dir.get_env(),
            Some(std::ffi::OsStr::new("MORTSOM_STACK_DIR"))
        );
    }

    #[test]
    fn stack_up_takes_an_image_or_a_mostro_ref_but_never_both() {
        use clap::Parser;
        let parse = |args: &[&str]| {
            let mut argv = vec!["mortsom", "stack", "up"];
            argv.extend_from_slice(args);
            super::Cli::try_parse_from(argv)
        };
        let pr = "https://github.com/MostroP2P/mostro/pull/963";
        for args in [
            &["--mostro-ref", "main"][..],
            &["--mostro-ref", "feature/my-change"][..],
            &["--mostro-ref", pr][..],
            &["--mostro-image", "mostrop2p/mostro:v0.18.8"][..],
            &[][..],
        ] {
            assert!(parse(args).is_ok(), "{args:?}");
        }
        for args in [
            &[
                "--mostro-ref",
                "main",
                "--mostro-image",
                "mostrop2p/mostro:v0.18.8",
            ][..],
            &["--mostro-ref", "https://github.com/MostroP2P/app/pull/1"][..],
            &[
                "--mostro-ref",
                "https://github.com/MostroP2P/mostro/pull/963/files",
            ][..],
            &["--mostro-ref", ""][..],
            &["--ref", "main"][..],
        ] {
            assert!(parse(args).is_err(), "{args:?}");
        }
        let cli = parse(&["--mostro-ref", pr]).unwrap();
        assert!(matches!(
            cli.command,
            super::Command::Stack {
                action: super::StackAction::Up { mostro_ref: Some(ref selector), mostro_image: None, .. }
            } if selector.git_ref() == "refs/pull/963/head"
        ));
    }

    #[test]
    fn a_bare_stack_up_builds_mostro_main() {
        use clap::Parser;

        let cli = super::Cli::try_parse_from(["mortsom", "stack", "up"]).unwrap();

        let super::Command::Stack {
            action:
                super::StackAction::Up {
                    mostro_image,
                    mostro_ref,
                    ..
                },
        } = cli.command
        else {
            panic!("not `stack up`");
        };
        assert_eq!(
            crate::stack::DaemonSource::from_options(mostro_image, mostro_ref),
            crate::stack::DaemonSource::Ref(crate::mostro_source::Selector::main())
        );
    }

    #[test]
    fn stack_up_replaces_a_foreign_stack_only_when_asked() {
        use clap::Parser;
        for (args, expected) in [
            (&["mortsom", "stack", "up"][..], false),
            (&["mortsom", "stack", "up", "--replace"][..], true),
        ] {
            let cli = super::Cli::try_parse_from(args).unwrap();
            assert!(matches!(
                cli.command,
                super::Command::Stack {
                    action: super::StackAction::Up { replace, .. }
                } if replace == expected
            ));
        }
    }

    #[test]
    fn recovery_abandon_takes_only_a_known_reason_and_a_confirmation() {
        use clap::Parser;
        let cli = super::Cli::try_parse_from([
            "mortsom",
            "recovery",
            "abandon",
            "run-1",
            "--reason",
            "stack-destroyed",
            "--confirm-run",
            "run-1",
        ])
        .unwrap();
        assert!(matches!(
            cli.command,
            super::Command::Recovery {
                action: super::RecoveryAction::Abandon { ref run, ref confirm_run, .. }
            } if run == "run-1" && confirm_run == "run-1"
        ));
        for args in [
            &[
                "mortsom",
                "recovery",
                "abandon",
                "run-1",
                "--confirm-run",
                "run-1",
            ][..],
            &[
                "mortsom",
                "recovery",
                "abandon",
                "run-1",
                "--reason",
                "stack-destroyed",
            ][..],
            &[
                "mortsom",
                "recovery",
                "abandon",
                "run-1",
                "--reason",
                "because",
                "--confirm-run",
                "run-1",
            ][..],
        ] {
            assert!(super::Cli::try_parse_from(args).is_err(), "{args:?}");
        }
    }

    #[test]
    fn stack_up_bonds_the_daemon_only_when_asked() {
        use clap::Parser;
        for (args, expected) in [
            (&["mortsom", "stack", "up"][..], false),
            (&["mortsom", "stack", "up", "--bond"][..], true),
        ] {
            let cli = super::Cli::try_parse_from(args).unwrap();
            assert!(matches!(
                cli.command,
                super::Command::Stack {
                    action: super::StackAction::Up { bond, .. }
                } if bond == expected
            ));
        }
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
