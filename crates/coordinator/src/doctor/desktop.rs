//! Read-only prerequisites for clients attached to the persistent regtest stack.
use std::path::{Path, PathBuf};

use mortsom_android_driver::process::CommandRunner;
use mortsom_desktop_driver::Platform;

use super::{CheckStatus, DoctorReport, HostProbe, NetProbes, net, report_config};
use crate::{Config, stack::StackContext};

/// Requirements of one platform-aware diagnostic invocation.
pub struct DesktopDoctorOptions {
    /// Linux or Web client target.
    pub platform: Platform,
    /// Existing native executable or Web bundle, avoiding source/build checks.
    pub artifact: Option<PathBuf>,
    /// Suppress stack, relay and wallet network queries.
    pub no_network: bool,
}

/// Checks the selected desktop platform without launching a client or paying.
pub async fn run_desktop_doctor(
    config_path: &Path,
    checkout: &Path,
    options: &DesktopDoctorOptions,
    host: &dyn HostProbe,
    net: &dyn NetProbes,
    runner: &dyn CommandRunner,
) -> DoctorReport {
    let mut report = DoctorReport::default();
    check_tools(
        options.platform,
        options.artifact.is_none(),
        host,
        &mut report,
    );
    let stack = if options.no_network {
        None
    } else {
        match StackContext::resolve(checkout, runner).await {
            Ok(stack) => {
                report.push("ortsom-stack", CheckStatus::Pass, "public status is ready");
                Some(stack)
            }
            Err(error) => {
                report.push("ortsom-stack", CheckStatus::Fail, error.to_string());
                None
            }
        }
    };
    let mut diagnosis = Config::diagnose(config_path, |name| {
        stack
            .as_ref()
            .and_then(|s| s.lookup(name))
            .or_else(|| host.env(name))
    });
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
        check_app(
            config,
            options.platform,
            options.artifact.as_deref(),
            host,
            &mut report,
        );
        if options.artifact.is_none() {
            super::desktop_build::check(
                Path::new(&config.mobile.source_dir),
                options.platform,
                host,
                &mut report,
            )
            .await;
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

fn check_tools(
    platform: Platform,
    needs_build: bool,
    host: &dyn HostProbe,
    report: &mut DoctorReport,
) {
    let tools: &[&str] = if needs_build {
        &["docker", "flutter", "cargo"]
    } else {
        &["docker"]
    };
    for tool in tools {
        check_tool(tool, None, host, report);
    }
    let tools: &[(&str, Option<&str>)] = match platform {
        Platform::Linux => &[
            ("Xvfb", None),
            ("dbus-daemon", None),
            ("gnome-keyring-daemon", None),
            ("import", None),
            ("/usr/libexec/at-spi-bus-launcher", None),
            ("/usr/libexec/at-spi2-registryd", None),
        ],
        Platform::Web => &[
            ("chromedriver", Some("MORTSOM_CHROMEDRIVER")),
            ("chromium", Some("MORTSOM_BROWSER")),
        ],
        Platform::Android => &[],
    };
    for (tool, variable) in tools {
        check_tool(tool, *variable, host, report);
    }
}

fn check_tool(tool: &str, variable: Option<&str>, host: &dyn HostProbe, report: &mut DoctorReport) {
    let configured = variable.and_then(|key| host.env(key));
    let program = configured.as_deref().unwrap_or(tool);
    let path = if Path::new(program).is_absolute() {
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

fn check_app(
    config: &Config,
    platform: Platform,
    artifact: Option<&Path>,
    host: &dyn HostProbe,
    report: &mut DoctorReport,
) {
    if config.mobile.profile != "mobile-v2" {
        report.push(
            "app-profile",
            CheckStatus::Fail,
            "Linux and Web require mobile-v2",
        );
    }
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
) -> Option<String> {
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
    let version = if let Some(key) = config.mobile.mostro_pubkey() {
        net::check_daemon(net, &config.mostro.relays, key, None, report)
            .await
            .and_then(|info| info.tag("mostro_version").map(str::to_owned))
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
    version
}

/// Read-only daemon and wallet facts required before desktop clients start.
pub struct DesktopPreflight {
    /// Version observed in the daemon's signed info event.
    pub daemon_version: Option<String>,
    /// Checks retained in the run's evidence.
    pub checks: DoctorReport,
}

/// Requires observed protocol/version/readiness, without any payment or order operation.
pub async fn desktop_preflight(config: &Config, net: &dyn NetProbes) -> DesktopPreflight {
    let mut checks = DoctorReport::default();
    let mut daemon_version = check_network(config, net, &mut checks).await;
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
        checks,
    }
}
