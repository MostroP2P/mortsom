//! `mortsom device smoke` (spec Phase 2.5): bring two clean devices up,
//! install the specified APK on both, launch it, record the initial screen
//! of each actor and tear everything down. No identity, no order, no
//! payment.

use std::path::{Path, PathBuf};
use std::time::Duration;

use mortsom_android_driver::adb::PackageVersion;
use mortsom_android_driver::avd::Actor;
use mortsom_android_driver::farm::{DeviceFarm, FarmSpec, TeardownItem};
use mortsom_android_driver::process::CommandRunner;
use mortsom_android_driver::sdk::{AndroidSdk, avd_home};
use mortsom_android_driver::ui::UiDriver;
use mortsom_common::check::{CheckStatus, DoctorReport};
use mortsom_common::logging::attach_run_log;
use mortsom_common::{Lane, Redactor, RunId};
use serde::{Deserialize, Serialize};
use tracing::{error, info};

use crate::artifacts::RunArtifacts;
use crate::config::Config;
use crate::report::{RunReport, write_report};

/// Step name under which the launch screen is recorded.
pub const LAUNCH_STEP: &str = "01-launch";
/// How long the app may take to show any UI after activation.
pub const LAUNCH_TIMEOUT: Duration = Duration::from_secs(120);
/// Grace period for emulator shutdown.
pub const SHUTDOWN_GRACE: Duration = Duration::from_secs(20);

/// Timing knobs; defaults suit real devices, tests shrink them.
#[derive(Debug, Clone)]
pub struct DeviceSmokeOptions {
    /// How long the app may take to reach the foreground.
    pub launch_timeout: Duration,
    /// Grace period for graceful emulator shutdown.
    pub shutdown_grace: Duration,
}

impl Default for DeviceSmokeOptions {
    fn default() -> Self {
        Self {
            launch_timeout: LAUNCH_TIMEOUT,
            shutdown_grace: SHUTDOWN_GRACE,
        }
    }
}

/// Machine-readable outcome, written to `summary.json`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceSmokeSummary {
    /// Run identifier.
    pub run_id: RunId,
    /// Lane.
    pub lane: Lane,
    /// Command name.
    pub command: String,
    /// App profile.
    pub app_profile: String,
    /// Installed app version (identical on both devices).
    pub app_version: Option<PackageVersion>,
    /// Checks performed.
    pub checks: DoctorReport,
    /// Teardown outcome.
    pub teardown: Vec<TeardownItem>,
    /// True when every check passed and teardown released everything.
    pub passed: bool,
}

/// Runs the device smoke.
pub async fn run_device_smoke(
    config: &Config,
    options: &DeviceSmokeOptions,
    runner: &dyn CommandRunner,
    ui: &dyn UiDriver,
    adapter: &dyn mortsom_app_contract::AppAdapter,
    env: &dyn Fn(&str) -> Option<String>,
) -> Result<DeviceSmokeSummary, String> {
    config.require_android().map_err(|e| e.to_string())?;
    let run_id = RunId::new();
    let redactor = Redactor::new().with_secrets(config.wallets.secrets());
    let artifacts = RunArtifacts::create(
        Path::new(&config.run.artifacts_dir),
        run_id.clone(),
        redactor.clone(),
    )
    .map_err(|e| format!("cannot create artifacts: {e}"))?;
    attach_run_log(artifacts.dir(), redactor).map_err(|e| format!("cannot attach run log: {e}"))?;
    info!(run = %run_id, lane = %config.run.lane, "device smoke starting");
    let mut summary = DeviceSmokeSummary {
        run_id,
        lane: config.run.lane,
        command: "device smoke".into(),
        app_profile: config.mobile.profile.clone(),
        app_version: None,
        checks: DoctorReport::default(),
        teardown: Vec::new(),
        passed: false,
    };

    // The same resolution every other command uses: a prebuilt APK when the
    // operator names one, otherwise a build of the configured checkout.
    let apk = crate::apk::resolve_apk(config, adapter, runner).await;
    let Some(spec) = prepare(config, options, apk, env, &artifacts, &mut summary) else {
        finish(&artifacts, &mut summary)?;
        return Ok(summary);
    };
    let sdk = match AndroidSdk::locate(env, |p| p.exists()) {
        Ok(sdk) => sdk,
        Err(e) => {
            summary
                .checks
                .push("android-sdk", CheckStatus::Fail, e.to_string());
            finish(&artifacts, &mut summary)?;
            return Ok(summary);
        }
    };
    let farm = match DeviceFarm::bring_up(runner, ui, &sdk, &spec).await {
        Ok(farm) => farm,
        Err(e) => {
            summary
                .checks
                .push("device-farm", CheckStatus::Fail, e.to_string());
            finish(&artifacts, &mut summary)?;
            return Ok(summary);
        }
    };
    summary.checks.push(
        "device-farm",
        CheckStatus::Pass,
        "alice and bob booted, same build installed",
    );
    summary.app_version = Some(farm.device(Actor::Alice).version.clone());

    for device in farm.devices() {
        record_launch(
            device,
            runner,
            options.launch_timeout,
            &artifacts,
            &mut summary,
        )
        .await;
    }
    let teardown = farm.tear_down(runner).await;
    record_teardown(teardown, &mut summary);
    summary.passed = summary.checks.is_healthy();
    finish(&artifacts, &mut summary)?;
    Ok(summary)
}

fn record_teardown(teardown: Vec<TeardownItem>, summary: &mut DeviceSmokeSummary) {
    let released = teardown.iter().filter(|i| i.ok).count();
    let all = teardown.iter().all(|i| i.ok);
    summary.checks.push(
        "device-teardown",
        if all {
            CheckStatus::Pass
        } else {
            CheckStatus::Fail
        },
        format!("{released}/{} resources released", teardown.len()),
    );
    summary.teardown = teardown;
}

/// Validates prerequisites that need no device and builds the farm spec.
fn prepare(
    config: &Config,
    options: &DeviceSmokeOptions,
    apk: Result<PathBuf, String>,
    env: &dyn Fn(&str) -> Option<String>,
    artifacts: &RunArtifacts,
    summary: &mut DeviceSmokeSummary,
) -> Option<FarmSpec> {
    let apk = match apk {
        Ok(apk) => apk,
        Err(e) => {
            summary.checks.push("apk", CheckStatus::Fail, e);
            return None;
        }
    };
    summary
        .checks
        .push("apk", CheckStatus::Pass, apk.display().to_string());
    let Some(avd_home) = avd_home(env) else {
        summary.checks.push(
            "avd-home",
            CheckStatus::Fail,
            "cannot resolve the AVD home (set ANDROID_AVD_HOME or HOME)",
        );
        return None;
    };
    Some(FarmSpec {
        android: config.android.as_ref()?.clone(),
        apk,
        package: config.mobile.package.clone(),
        main_activity: (!config.mobile.main_activity.is_empty())
            .then(|| config.mobile.main_activity.clone()),
        avd_home,
        log_dir: artifacts.dir().to_path_buf(),
        shutdown_grace: options.shutdown_grace,
    })
}

/// Launches the app on one device and records its initial screen.
async fn record_launch(
    device: &mortsom_android_driver::farm::Device,
    runner: &dyn CommandRunner,
    launch_timeout: Duration,
    artifacts: &RunArtifacts,
    summary: &mut DeviceSmokeSummary,
) {
    let actor = device.actor.as_str();
    let name = format!("{actor} launch");
    if let Err(e) = device.session.clear_app().await {
        summary
            .checks
            .push(name, CheckStatus::Fail, format!("clear app data: {e}"));
        return;
    }
    if let Err(e) = device.session.activate_app().await {
        summary
            .checks
            .push(name, CheckStatus::Fail, format!("activate app: {e}"));
        return;
    }
    let foreground = wait_foreground(device, runner, launch_timeout).await;
    let recorded = capture_evidence(device, runner, artifacts).await;
    match (foreground, recorded) {
        (true, true) => summary.checks.push(
            name,
            CheckStatus::Pass,
            "app in foreground; screen and hierarchy recorded",
        ),
        (true, false) => summary.checks.push(
            name,
            CheckStatus::Fail,
            "app in foreground but evidence capture failed",
        ),
        (false, _) => summary.checks.push(
            name,
            CheckStatus::Fail,
            format!("app not in foreground within {launch_timeout:?}"),
        ),
    }
}

/// Writes hierarchy, screenshot and logcat for the launch step. Returns true
/// when both UI captures succeeded.
async fn capture_evidence(
    device: &mortsom_android_driver::farm::Device,
    runner: &dyn CommandRunner,
    artifacts: &RunArtifacts,
) -> bool {
    let actor = device.actor.as_str();
    let source = device.session.page_source().await;
    let screenshot = device.session.screenshot_png().await;
    match &source {
        Ok(xml) => {
            if let Err(e) = artifacts.write_step_text(actor, LAUNCH_STEP, "ui.xml", xml) {
                error!(error = %e, "cannot write hierarchy");
            }
        }
        Err(e) => error!(actor, error = %e, "cannot dump hierarchy"),
    }
    match &screenshot {
        Ok(png) => {
            if let Err(e) = artifacts.write_step_bytes(actor, LAUNCH_STEP, "screen.png", png) {
                error!(error = %e, "cannot write screenshot");
            }
        }
        Err(e) => error!(actor, error = %e, "cannot take screenshot"),
    }
    match device.emulator.adb().logcat_dump(runner).await {
        Ok(logcat) => {
            if let Err(e) = artifacts.write_text(&format!("device-logcat-{actor}.txt"), &logcat) {
                error!(error = %e, "cannot write logcat");
            }
        }
        Err(e) => error!(actor, error = %e, "cannot dump logcat"),
    }
    source.is_ok() && screenshot.is_ok()
}

/// Bounded wait until the app's package is the resumed activity.
async fn wait_foreground(
    device: &mortsom_android_driver::farm::Device,
    runner: &dyn CommandRunner,
    timeout: Duration,
) -> bool {
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        let resumed = device
            .emulator
            .adb()
            .shell(runner, &["dumpsys", "activity", "activities"])
            .await
            .map(|o| is_resumed(&o.stdout, &device.spec.app_package))
            .unwrap_or_else(|e| {
                tracing::warn!(error = %e, "dumpsys activity failed while waiting for foreground");
                false
            });
        if resumed {
            return true;
        }
        if tokio::time::Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
}

/// True when `dumpsys activity activities` reports `package` as the resumed
/// activity. Android 12+ prints `topResumedActivity`; older releases print
/// `mResumedActivity`; both are accepted.
pub fn is_resumed(dumpsys: &str, package: &str) -> bool {
    dumpsys.lines().any(|l| {
        (l.contains("topResumedActivity") || l.contains("mResumedActivity")) && l.contains(package)
    })
}

fn finish(artifacts: &RunArtifacts, summary: &mut DeviceSmokeSummary) -> Result<(), String> {
    let environment = serde_json::json!({
        "run_id": summary.run_id,
        "lane": summary.lane,
        "command": summary.command,
        "app_profile": summary.app_profile,
        "app_version": summary.app_version,
    });
    let report = RunReport {
        stack: None,
        app_artifact: None,
        run_id: &summary.run_id,
        lane: summary.lane,
        command: &summary.command,
        app_profile: Some(&summary.app_profile),
        checks: &summary.checks,
        passed: summary.passed,
    };
    write_report(
        artifacts,
        &report,
        summary,
        &environment,
        &summary.teardown,
        true,
    )
}

#[cfg(test)]
mod tests;
