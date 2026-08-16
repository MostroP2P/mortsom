//! `mortsom app setup` (spec Phase 3 exit gate): using only UI actions, both
//! apps end with distinct identities that survive a restart, connected NWC
//! wallets, the local relay and the local daemon configured. Moves no value.

use std::path::{Path, PathBuf};
use std::time::Duration;

use mortsom_android_driver::farm::{DeviceFarm, FarmSpec, TeardownItem};
use mortsom_android_driver::process::CommandRunner;
use mortsom_android_driver::sdk::{AndroidSdk, avd_home};
use mortsom_android_driver::ui::UiDriver;
use mortsom_app_contract::{AppAdapter, Identity, TestEnvironment};
use mortsom_common::check::{CheckStatus, DoctorReport};
use mortsom_common::logging::attach_run_log;
use mortsom_common::{Lane, Redactor, RunId};
use mortsom_environment::docker::ContainerRuntime;
use mortsom_environment::relay::{
    ReadinessProbe, RelayHandle, RelayTeardown, start_relay, teardown_relay,
};
use serde::{Deserialize, Serialize};
use tracing::{error, info};

use crate::artifacts::RunArtifacts;
use crate::bootstrap::{ActorSetup, bootstrap_actor};
use crate::config::Config;
use crate::evidence::StepRecorder;
use crate::report::{RunReport, write_report};

/// Grace for emulator shutdown.
pub const SHUTDOWN_GRACE: Duration = Duration::from_secs(20);

/// Machine-readable outcome, written to `summary.json`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppSetupSummary {
    /// Run identifier.
    pub run_id: RunId,
    /// Lane.
    pub lane: Lane,
    /// Command.
    pub command: String,
    /// App profile.
    pub app_profile: String,
    /// Relay used.
    pub relay: Option<RelayHandle>,
    /// Actor outcomes.
    pub actors: Vec<ActorSetup>,
    /// Checks.
    pub checks: DoctorReport,
    /// Farm teardown.
    pub farm_teardown: Vec<TeardownItem>,
    /// Relay teardown.
    pub relay_teardown: Option<RelayTeardown>,
    /// True when every check passed.
    pub passed: bool,
}

/// Timing knobs.
#[derive(Debug, Clone)]
pub struct AppSetupOptions {
    /// Grace for emulator shutdown.
    pub shutdown_grace: Duration,
}

impl Default for AppSetupOptions {
    fn default() -> Self {
        Self {
            shutdown_grace: SHUTDOWN_GRACE,
        }
    }
}

/// Everything the setup needs from the outside world.
pub struct SetupDeps<'a> {
    /// Process runner.
    pub runner: &'a dyn CommandRunner,
    /// UI driver.
    pub ui: &'a dyn UiDriver,
    /// Container runtime.
    pub docker: &'a dyn ContainerRuntime,
    /// Relay readiness probe.
    pub probe: &'a dyn ReadinessProbe,
    /// Application adapter.
    pub adapter: &'a dyn AppAdapter,
    /// Environment lookup.
    pub env: &'a dyn Fn(&str) -> Option<String>,
}

/// Runs the setup end to end.
pub async fn run_app_setup(
    config: &Config,
    options: &AppSetupOptions,
    deps: &SetupDeps<'_>,
) -> Result<AppSetupSummary, String> {
    let run_id = RunId::new();
    let redactor = Redactor::new()
        .with_secret(&config.wallets.alice_nwc)
        .with_secret(&config.wallets.bob_nwc);
    let artifacts = RunArtifacts::create(
        Path::new(&config.run.artifacts_dir),
        run_id.clone(),
        redactor.clone(),
    )
    .map_err(|e| format!("cannot create artifacts: {e}"))?;
    attach_run_log(artifacts.dir(), redactor).map_err(|e| format!("cannot attach run log: {e}"))?;
    info!(run = %run_id, lane = %config.run.lane, "app setup starting");
    let mut summary = AppSetupSummary {
        run_id: run_id.clone(),
        lane: config.run.lane,
        command: "app setup".into(),
        app_profile: config.mobile.profile.clone(),
        relay: None,
        actors: Vec::new(),
        checks: DoctorReport::default(),
        farm_teardown: Vec::new(),
        relay_teardown: None,
        passed: false,
    };

    let relay_timeout = Duration::from_secs(config.mostro.health_timeout_secs);
    let relay = match start_relay(
        deps.docker,
        deps.probe,
        &config.relay,
        run_id.as_str(),
        artifacts.dir(),
        relay_timeout,
    )
    .await
    {
        Ok(handle) => handle,
        Err(e) => {
            summary
                .checks
                .push("relay-start", CheckStatus::Fail, e.to_string());
            return finish(&artifacts, &mut summary).map(|()| summary);
        }
    };
    summary
        .checks
        .push("relay-start", CheckStatus::Pass, relay.host_url());
    summary.relay = Some(relay.clone());
    let env = TestEnvironment {
        mostro_pubkey: config.mobile.mostro_pubkey.clone(),
        relay_url: relay.emulator_url(),
        lane: config.run.lane,
    };

    let farm = match bring_up_farm(config, options, deps, &artifacts, &mut summary).await {
        Some(farm) => farm,
        None => {
            record_relay_teardown(deps.docker, &relay, &artifacts, &mut summary).await;
            return finish(&artifacts, &mut summary).map(|()| summary);
        }
    };
    let recorder = StepRecorder::default();
    for device in farm.devices() {
        let actor = bootstrap_actor(
            config,
            deps.adapter,
            device,
            &env,
            &artifacts,
            &recorder,
            &mut summary.checks,
        )
        .await;
        summary.actors.push(actor);
    }
    check_identities(&mut summary);
    summary.farm_teardown = farm.tear_down(deps.runner).await;
    let farm_ok = summary.farm_teardown.iter().all(|i| i.ok);
    summary.checks.push(
        "device-teardown",
        if farm_ok {
            CheckStatus::Pass
        } else {
            CheckStatus::Fail
        },
        format!("{} items", summary.farm_teardown.len()),
    );
    record_relay_teardown(deps.docker, &relay, &artifacts, &mut summary).await;
    finish(&artifacts, &mut summary).map(|()| summary)
}

async fn bring_up_farm(
    config: &Config,
    options: &AppSetupOptions,
    deps: &SetupDeps<'_>,
    artifacts: &RunArtifacts,
    summary: &mut AppSetupSummary,
) -> Option<DeviceFarm> {
    let apk = match resolve_apk(config, deps).await {
        Ok(apk) => apk,
        Err(e) => {
            summary.checks.push("apk", CheckStatus::Fail, e);
            return None;
        }
    };
    summary
        .checks
        .push("apk", CheckStatus::Pass, apk.display().to_string());
    let Some(avd_home) = avd_home(deps.env) else {
        summary
            .checks
            .push("avd-home", CheckStatus::Fail, "cannot resolve the AVD home");
        return None;
    };
    let sdk = match AndroidSdk::locate(deps.env, |p| p.exists()) {
        Ok(sdk) => sdk,
        Err(e) => {
            summary
                .checks
                .push("android-sdk", CheckStatus::Fail, e.to_string());
            return None;
        }
    };
    let spec = FarmSpec {
        android: config.android.clone(),
        apk,
        package: config.mobile.package.clone(),
        main_activity: (!config.mobile.main_activity.is_empty())
            .then(|| config.mobile.main_activity.clone()),
        avd_home,
        log_dir: artifacts.dir().to_path_buf(),
        shutdown_grace: options.shutdown_grace,
    };
    match DeviceFarm::bring_up(deps.runner, deps.ui, &sdk, &spec).await {
        Ok(farm) => {
            summary.checks.push(
                "device-farm",
                CheckStatus::Pass,
                "alice and bob booted, same build installed",
            );
            Some(farm)
        }
        Err(e) => {
            summary
                .checks
                .push("device-farm", CheckStatus::Fail, e.to_string());
            None
        }
    }
}

/// Uses the configured APK or builds one through the adapter.
async fn resolve_apk(config: &Config, deps: &SetupDeps<'_>) -> Result<PathBuf, String> {
    crate::apk::resolve_apk(config, deps.adapter, deps.runner).await
}

fn check_identities(summary: &mut AppSetupSummary) {
    let ids: Vec<&Identity> = summary
        .actors
        .iter()
        .filter_map(|a| a.identity.as_ref())
        .collect();
    match ids.as_slice() {
        [alice, bob] if alice != bob => summary.checks.push(
            "identities-differ",
            CheckStatus::Pass,
            "alice and bob have distinct identities",
        ),
        [_, _] => summary.checks.push(
            "identities-differ",
            CheckStatus::Fail,
            "alice and bob show the same identity",
        ),
        _ => summary.checks.push(
            "identities-differ",
            CheckStatus::Skip,
            "not both actors onboarded",
        ),
    }
}

async fn record_relay_teardown(
    docker: &dyn ContainerRuntime,
    relay: &RelayHandle,
    artifacts: &RunArtifacts,
    summary: &mut AppSetupSummary,
) {
    let (teardown, logs) = teardown_relay(docker, relay, false).await;
    if let Err(e) = artifacts.write_text("relay-container.log", &logs) {
        error!(error = %e, "cannot write relay log");
    }
    summary.checks.push(
        "relay-teardown",
        if teardown.removed {
            CheckStatus::Pass
        } else {
            CheckStatus::Fail
        },
        teardown
            .error
            .clone()
            .unwrap_or_else(|| "container removed".into()),
    );
    summary.relay_teardown = Some(teardown);
}

fn finish(artifacts: &RunArtifacts, summary: &mut AppSetupSummary) -> Result<(), String> {
    summary.passed = summary.checks.is_healthy();
    let cleanup = serde_json::json!({
        "farm": summary.farm_teardown,
        "relay": summary.relay_teardown,
    });
    let environment = serde_json::json!({
        "run_id": summary.run_id,
        "lane": summary.lane,
        "command": summary.command,
        "app_profile": summary.app_profile,
        "relay": summary.relay.as_ref().map(|r| serde_json::json!({"container_name": r.container_name, "host_port": r.host_port})),
    });
    let report = RunReport {
        run_id: &summary.run_id,
        lane: summary.lane,
        command: &summary.command,
        app_profile: Some(&summary.app_profile),
        checks: &summary.checks,
        passed: summary.passed,
    };
    write_report(artifacts, &report, summary, &environment, &cleanup, true)
}

#[cfg(test)]
mod tests;
