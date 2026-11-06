//! Read-only prerequisites for clients attached to the regtest stack.
use std::path::{Path, PathBuf};

use mortsom_desktop_driver::Platform;
use mortsom_ui_driver::process::CommandRunner;

use super::{CheckStatus, DoctorReport, HostProbe, NetProbes, net, report_config};
use crate::{Config, stack::StackContext};

/// Requirements of one platform-aware diagnostic invocation.
pub struct DesktopDoctorOptions {
    /// Linux or Web client target; the profile decides when absent.
    pub platform: Option<Platform>,
    /// `--profile`, overriding `mobile.profile`.
    pub profile: Option<String>,
    /// The mostro-cli to check with profile `cli`.
    pub cli: crate::mostro_cli_build::CliSelection,
    /// Existing native executable or Web bundle, avoiding source/build checks.
    pub artifact: Option<PathBuf>,
    /// CLI overrides of the configured source mode.
    pub source: crate::cli::SourceOverrides,
    /// Suppress stack, relay and wallet network queries.
    pub no_network: bool,
}

/// Checks the selected desktop platform without launching a client or paying.
pub async fn run_desktop_doctor(
    config_path: &Path,
    stack_dir: &Path,
    options: &DesktopDoctorOptions,
    host: &dyn HostProbe,
    net: &dyn NetProbes,
    runner: &dyn CommandRunner,
) -> DoctorReport {
    let mut report = DoctorReport::default();
    // The configured profile counts as much as `--profile`: read it before
    // the tools are checked, since they differ per client.
    let configured = configured_profile(config_path);
    let cli_requested = crate::mostro_cli::resolve_client(
        configured
            .as_deref()
            .unwrap_or(mortsom_app_contract::config::DEFAULT_PROFILE),
        options.profile.as_deref(),
        options.platform,
    )
    .is_ok_and(|(_, platform)| platform == Platform::Cli);
    if cli_requested {
        super::cli::check_tools(&options.cli, host, &mut report);
    } else {
        check_tools(
            options.platform.unwrap_or_default(),
            options.artifact.is_none(),
            Path::new("Xvfb"),
            host,
            &mut report,
        )
        .await;
    }
    let stack = if options.no_network {
        None
    } else {
        match StackContext::resolve(stack_dir, runner).await {
            Ok(stack) => {
                report.push("stack", CheckStatus::Pass, "ready");
                Some(stack)
            }
            Err(error) => {
                report.push("stack", CheckStatus::Fail, error.to_string());
                None
            }
        }
    };
    let mut diagnosis = Config::diagnose_with_source(
        config_path,
        |name| {
            stack
                .as_ref()
                .and_then(|s| s.lookup(name))
                .or_else(|| host.env(name))
        },
        options.source.selection().as_ref(),
    );
    if let (Some(stack), Some(config)) = (&stack, &mut diagnosis.config) {
        stack.configure_relay(config);
        match stack.snapshot().validate_config(config) {
            Ok(()) => report.push(
                "stack-config-binding",
                CheckStatus::Pass,
                "active roles match public stack metadata",
            ),
            Err(error) => report.push("stack-config-binding", CheckStatus::Fail, error.to_string()),
        }
    }
    report_config(config_path, &diagnosis, &mut report);
    if let Some(config) = &diagnosis.config {
        match crate::mostro_cli::resolve_client(
            &config.mobile.profile,
            options.profile.as_deref(),
            options.platform,
        ) {
            Err(error) => report.push("client", CheckStatus::Fail, error),
            Ok((_, Platform::Cli)) => super::cli::check(&options.cli, &mut report).await,
            Ok((_, platform)) => {
                check_app(
                    config,
                    platform,
                    options.artifact.as_deref(),
                    host,
                    &mut report,
                );
                if options.artifact.is_none() {
                    super::desktop_build::check(
                        (!config.mobile.source_dir.is_empty())
                            .then(|| Path::new(&config.mobile.source_dir)),
                        platform,
                        host,
                        &mut report,
                    )
                    .await;
                }
            }
        }
    }
    if options.no_network {
        report.push("network", CheckStatus::Skip, "--no-network");
    } else if let (Some(config), Some(stack)) = (&diagnosis.config, &stack)
        && stack.snapshot().validate_config(config).is_ok()
    {
        check_network(config, net, &mut report).await;
    } else {
        report.push(
            "network",
            CheckStatus::Skip,
            "stack identity and configuration did not validate",
        );
    }
    let redactor = mortsom_common::Redactor::default();
    for check in &mut report.checks {
        check.detail = redactor.redact(&check.detail);
    }
    report
}

/// `mobile.profile` as the configuration file names it, read without
/// resolving anything else; `None` when the file or the key is absent.
fn configured_profile(config_path: &Path) -> Option<String> {
    let text = std::fs::read_to_string(config_path).ok()?;
    let value: toml::Value = toml::from_str(&text).ok()?;
    value
        .get("mobile")?
        .get("profile")?
        .as_str()
        .map(str::to_owned)
}

async fn check_tools(
    platform: Platform,
    needs_build: bool,
    xvfb: &Path,
    host: &dyn HostProbe,
    report: &mut DoctorReport,
) {
    let tools: &[&str] = if needs_build {
        &["docker", "flutter", "cargo", "git"]
    } else {
        &["docker"]
    };
    for tool in tools {
        check_tool(tool, None, host, report);
    }
    if platform == Platform::Linux {
        check_tool(&xvfb.to_string_lossy(), None, host, report);
    }
    if platform == Platform::Linux {
        for tool in [
            "dbus-daemon",
            "gnome-keyring-daemon",
            "import",
            "/usr/libexec/at-spi2-registryd",
        ] {
            check_tool(tool, None, host, report);
        }
    }
    if platform == Platform::Web {
        let tools = crate::web_tools::resolve(host);
        let status = if tools.chromedriver.is_usable() {
            CheckStatus::Pass
        } else {
            CheckStatus::Fail
        };
        report.push("chromedriver", status, tools.chromedriver.describe());
        check_browser(&tools.browser, host, report).await;
    }
}

/// The browser must start, not only be found: Ubuntu's `chromium-browser`
/// package is a script that hands over to the Chromium snap and refuses to
/// run when the snap is gone. Asking its version proves it starts.
async fn check_browser(
    browser: &crate::web_tools::WebTool,
    host: &dyn HostProbe,
    report: &mut DoctorReport,
) {
    if !browser.is_usable() {
        report.push("chromium", CheckStatus::Fail, browser.describe());
        return;
    }
    let (status, answer) = match host.run(&browser.path, &["--version"]).await {
        Ok(output) if output.success => (CheckStatus::Pass, output.combined()),
        Ok(output) => (CheckStatus::Fail, output.combined()),
        Err(error) => (CheckStatus::Fail, error),
    };
    let answer = answer.trim();
    report.push(
        "chromium",
        status,
        format!("{}: {answer}", browser.describe()),
    );
}

fn check_tool(tool: &str, variable: Option<&str>, host: &dyn HostProbe, report: &mut DoctorReport) {
    let configured = variable.and_then(|key| host.env(key));
    let program = configured.as_deref().unwrap_or(tool);
    let path = if program.contains(std::path::MAIN_SEPARATOR) {
        host.is_executable(Path::new(program))
            .then(|| Path::new(program).to_path_buf())
    } else {
        host.which(program)
    };
    match path {
        Some(path) => report.push(tool, CheckStatus::Pass, path.display().to_string()),
        None => report.push(tool, CheckStatus::Fail, format!("{program} not found")),
    }
}

/// The app checks; reached only once the profile resolved to the app, so
/// the configured profile is no longer consulted here.
fn check_app(
    config: &Config,
    platform: Platform,
    artifact: Option<&Path>,
    host: &dyn HostProbe,
    report: &mut DoctorReport,
) {
    if let Some(path) = artifact {
        match crate::desktop::validate_artifact(platform, path, host) {
            Ok(()) => report.push(
                "app-artifact",
                CheckStatus::Pass,
                path.display().to_string(),
            ),
            Err(error) => report.push("app-artifact", CheckStatus::Fail, error),
        }
        return;
    }
    if config.mobile.source_dir.is_empty() {
        report.push(
            "app source",
            CheckStatus::Pass,
            format!(
                "managed {} at {}; run fetches a fresh head before building",
                crate::app_source::REPOSITORY,
                config.mobile.app_ref.as_deref().unwrap_or("main")
            ),
        );
        report.push(
            "app source contract",
            CheckStatus::Skip,
            "checked after fetch by run; doctor does not fetch or build source",
        );
        return;
    }
    for file in [
        "pubspec.yaml",
        "lib/main_mortsom.dart",
        "scripts/frb-generate.sh",
    ] {
        let path = Path::new(&config.mobile.source_dir).join(file);
        let status = if host.is_file(&path) {
            CheckStatus::Pass
        } else {
            CheckStatus::Fail
        };
        report.push(format!("app {file}"), status, path.display().to_string());
    }
}

async fn check_network(
    config: &Config,
    net: &dyn NetProbes,
    report: &mut DoctorReport,
) -> Option<mortsom_environment::daemon::DaemonInfo> {
    let status = if net.local_port(config.relay.host_port).await == super::PortState::InUse {
        CheckStatus::Pass
    } else {
        CheckStatus::Fail
    };
    report.push(
        "relay-port",
        status,
        format!("persistent relay port {}", config.relay.host_port),
    );
    let info = if let Some(key) = config.mobile.mostro_pubkey() {
        net::check_daemon(net, &config.mostro.relays, key, None, report).await
    } else {
        None
    };
    let liquidity = net::Liquidity {
        max_order_sats: config.run.max_order_sats,
        max_run_spend_sats: config.run.max_run_spend_sats,
    };
    for (actor, wallet) in [
        ("alice", &config.wallets.alice),
        ("bob", &config.wallets.bob),
    ] {
        if let Some(probe) = net::check_wallet(net, actor, wallet, liquidity, report).await {
            let ready = probe.network.as_deref() == Some("regtest")
                && ["pay_invoice", "make_invoice", "lookup_invoice"]
                    .iter()
                    .all(|required| probe.methods.iter().any(|method| method == required));
            report.push(
                format!("wallet {actor} capabilities"),
                if ready {
                    CheckStatus::Pass
                } else {
                    CheckStatus::Fail
                },
                "persistent stack requires regtest and pay_invoice/make_invoice/lookup_invoice",
            );
        }
    }
    info
}

/// Read-only daemon and wallet facts required before desktop clients start.
pub struct DesktopPreflight {
    /// Version observed in the daemon's signed info event.
    pub daemon_version: Option<String>,
    /// The daemon's signed info event, as observed: what it says it runs.
    pub daemon_info: Option<mortsom_environment::daemon::DaemonInfo>,
    /// Checks retained in the run's evidence.
    pub checks: DoctorReport,
}

/// Requires observed protocol/version/readiness, without any payment or order operation.
pub async fn desktop_preflight(config: &Config, net: &dyn NetProbes) -> DesktopPreflight {
    let mut checks = DoctorReport::default();
    let daemon_info = check_network(config, net, &mut checks).await;
    let mut daemon_version = daemon_info
        .as_ref()
        .and_then(|info| info.tag("mostro_version").map(str::to_owned));
    if daemon_version.as_deref().is_none_or(|value| {
        value.is_empty()
            || value.len() > 64
            || !value
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b".-+".contains(&byte))
    }) {
        checks.push(
            "mostro-version",
            CheckStatus::Fail,
            "daemon info has no valid version",
        );
    }
    let verified = ["mostro-daemon", "mostro-protocol"].iter().all(|name| {
        checks
            .checks
            .iter()
            .any(|check| check.name == *name && check.status == CheckStatus::Pass)
    }) && !checks
        .checks
        .iter()
        .any(|check| check.name == "mostro-version" && check.status == CheckStatus::Fail);
    if !verified {
        daemon_version = None;
    }
    // A standalone doctor may describe incomplete observations as warnings;
    // a value-moving run requires each readiness fact to be established.
    for check in &mut checks.checks {
        if matches!(check.status, CheckStatus::Skip | CheckStatus::Warn) {
            check.status = CheckStatus::Fail;
        }
    }
    DesktopPreflight {
        daemon_version,
        daemon_info,
        checks,
    }
}

/// Checks selected runtime and build requirements before any expensive generation/build.
pub(crate) async fn require_build_prerequisites(
    config: &Config,
    platform: Platform,
    xvfb: &Path,
    host: &dyn HostProbe,
) -> Result<(), String> {
    let mut report = DoctorReport::default();
    check_tools(platform, true, xvfb, host, &mut report).await;
    check_app(config, platform, None, host, &mut report);
    super::desktop_build::check(
        Some(Path::new(&config.mobile.source_dir)),
        platform,
        host,
        &mut report,
    )
    .await;
    if report.is_healthy() {
        Ok(())
    } else {
        Err(mortsom_common::Redactor::default().redact(&format!(
            "desktop build prerequisites failed before generation:\n{}",
            report.render_with_color(false)
        )))
    }
}

/// Checks installed tools before fetching remote source.
pub(crate) async fn require_tools(
    platform: Platform,
    xvfb: &Path,
    host: &dyn HostProbe,
) -> Result<(), String> {
    let mut report = DoctorReport::default();
    check_tools(platform, true, xvfb, host, &mut report).await;
    if report.is_healthy() {
        Ok(())
    } else {
        Err(format!(
            "app preparation prerequisites failed:\n{}",
            report.render_with_color(false)
        ))
    }
}
