//! `mortsom cleanup <run>` (spec Phase 5.5): after a crashed or interrupted
//! run, remove what that run left on this host — its relay container and
//! Mortsom's emulators — and report what it cannot fix: orders the run's
//! own sweep left open. It never claims more than it verified.

use std::path::{Path, PathBuf};

use mortsom_android_driver::adb::Adb;
use mortsom_android_driver::avd::Actor;
use mortsom_android_driver::emulator::DeviceSlot;
use mortsom_android_driver::process::{CommandRunner, run_ok};
use mortsom_android_driver::sdk::AndroidSdk;
use mortsom_android_driver::webdriver::server_ready;
use mortsom_common::RunId;
use mortsom_environment::docker::ContainerRuntime;
use mortsom_scenario_kit::cleanup::CleanupReport;

/// How long the orphan probe waits for an answer on the Appium port.
const APPIUM_PROBE: std::time::Duration = std::time::Duration::from_secs(2);
/// Deadline for the command that stops it.
const APPIUM_KILL_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);
use serde::{Deserialize, Serialize};
use tracing::{info, warn};

use crate::artifacts::files;
use crate::config::MaintenanceConfig as Config;

pub mod lookalike;
mod platform;

/// File the command writes into the run directory when it exists.
pub const CLEANUP_COMMAND_FILE: &str = "cleanup-command.json";

/// An Appium server found still listening on the configured port.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppiumItem {
    /// Port it was answering on.
    pub port: u16,
    /// Whether it is gone now.
    pub stopped: bool,
    /// Why it could not be stopped.
    pub error: Option<String>,
}

/// What happened to one relay container.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContainerItem {
    /// Container name.
    pub name: String,
    /// True when it is gone now.
    pub removed: bool,
    /// Error when it is not.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// What the command did with one emulator slot.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum EmulatorAction {
    /// Nothing answered on the slot's serial.
    NotRunning,
    /// An emulator was running on the slot but it is not Mortsom's AVD; it
    /// was left alone.
    Foreign,
    /// Mortsom's emulator was asked to shut down.
    Killed,
    /// The shutdown request failed.
    Failed,
    /// The Android SDK could not be located, so nothing was inspected.
    NoSdk,
}

/// One emulator slot.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EmulatorItem {
    /// Actor the slot belongs to.
    pub actor: Actor,
    /// adb serial.
    pub serial: String,
    /// AVD name found on the serial, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub avd_name: Option<String>,
    /// Outcome.
    pub action: EmulatorAction,
    /// Error detail for `Failed`/`NoSdk`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Machine-readable outcome.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum DeviceCleanupStatus {
    /// Older report with no recorded platform check.
    #[default]
    Unknown,
    /// Android resource checks ran; individual results may still be failures.
    AndroidChecked,
    /// Desktop recovery is not available in this command yet.
    DesktopUnsupported,
}

/// Machine-readable outcome.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CleanupRunSummary {
    /// Run being cleaned.
    pub run_id: String,
    /// Whether `<artifacts_dir>/<run_id>` exists.
    pub run_dir_found: bool,
    /// Whether the run wrote `cleanup.json`.
    pub cleanup_report_found: bool,
    /// Orders the run's own sweep left open; only a device session can
    /// close them, so they are reported, not fixed.
    pub unresolved_orders: Vec<String>,
    /// Relay containers of this run.
    pub containers: Vec<ContainerItem>,
    /// Emulator slots.
    pub emulators: Vec<EmulatorItem>,
    /// The Appium server a crashed run left listening, if any.
    pub appium: Option<AppiumItem>,
    /// Whether device cleanup was attempted on a supported platform.
    #[serde(default)]
    pub device_cleanup: DeviceCleanupStatus,
    /// True when nothing of this run is known to remain on the host and no
    /// order is unresolved.
    pub clean: bool,
}

impl CleanupRunSummary {
    /// Human-readable rendering.
    pub fn render(&self) -> String {
        let mut out = format!("cleanup of run {}\n", self.run_id);
        if !self.run_dir_found {
            out.push_str(
                "  run directory: not found; orders UNKNOWN (host resources still checked)\n",
            );
        } else if !self.cleanup_report_found {
            out.push_str(
                "  cleanup.json: missing; the run did not reach its sweep, orders UNKNOWN: check the maker's app\n",
            );
        }
        for order in &self.unresolved_orders {
            out.push_str(&format!(
                "  UNRESOLVED order {order}: cancel it from the maker's app; this command cannot\n"
            ));
        }
        if self.containers.is_empty() {
            out.push_str("  relay containers: none\n");
        }
        for c in &self.containers {
            match &c.error {
                None => out.push_str(&format!("  relay container {}: removed\n", c.name)),
                Some(e) => out.push_str(&format!(
                    "  relay container {}: NOT removed ({e})\n",
                    c.name
                )),
            }
        }
        match &self.appium {
            None if self.device_cleanup == DeviceCleanupStatus::DesktopUnsupported => out.push_str(
                "  device cleanup: NOT verified; desktop recovery is not implemented; Android/Appium probes skipped\n"),
            None if self.device_cleanup == DeviceCleanupStatus::Unknown => out.push_str(
                "  device cleanup: UNKNOWN; recorded platform could not be verified; device probes skipped\n"),
            None => out.push_str("  appium server: none left behind\n"),
            Some(a) if a.stopped => {
                out.push_str(&format!("  appium server on port {}: stopped\n", a.port))
            }
            Some(a) => out.push_str(&format!(
                "  appium server on port {}: NOT stopped ({})\n",
                a.port,
                a.error.as_deref().unwrap_or("still answering")
            )),
        }
        for e in &self.emulators {
            let detail = match (&e.avd_name, &e.error) {
                (_, Some(err)) => format!(" ({err})"),
                (Some(name), None) => format!(" ({name})"),
                (None, None) => String::new(),
            };
            out.push_str(&format!(
                "  emulator {} on {}: {:?}{detail}\n",
                e.actor, e.serial, e.action
            ));
        }
        out.push_str(if self.clean {
            "result: clean\n"
        } else {
            "result: NOT clean (see above)\n"
        });
        out
    }
}

/// External dependencies.
pub struct CleanupDeps<'a> {
    /// Process runner (adb).
    pub runner: &'a dyn CommandRunner,
    /// Container runtime.
    pub docker: &'a dyn ContainerRuntime,
    /// Environment lookup (SDK location).
    pub env: &'a dyn Fn(&str) -> Option<String>,
}

/// Cleans up after `run_id` (validated: a single safe path segment).
pub async fn cleanup_run(
    config: &Config,
    run_id: &RunId,
    deps: &CleanupDeps<'_>,
) -> CleanupRunSummary {
    let run_id = run_id.as_str();
    let run_dir = Path::new(&config.run.artifacts_dir).join(run_id);
    let run_dir_found = run_dir.is_dir();
    let (cleanup_report_found, unresolved_orders) = read_unresolved(&run_dir);
    let device_cleanup = platform::resolve(&run_dir, config.android.is_some());
    let android_cleanup = device_cleanup == DeviceCleanupStatus::AndroidChecked;
    let containers = if android_cleanup {
        remove_containers(deps.docker, config, run_id).await
    } else {
        Vec::new()
    };
    let emulators = if android_cleanup {
        shut_down_emulators(deps, config).await
    } else {
        Vec::new()
    };
    // Mortsom starts the Appium server and stops it on teardown; a run that
    // died before teardown leaves it holding the port, and the next run
    // cannot start its own.
    let appium = match (&config.android, android_cleanup) {
        (Some(android), true) => stop_orphan_appium(deps.runner, android.appium_port).await,
        _ => None,
    };
    // Orders count as verified only when the run's own sweep reported them;
    // a run that died before its sweep leaves them unknown, not clean.
    let clean = android_cleanup
        && cleanup_report_found
        && unresolved_orders.is_empty()
        && containers.iter().all(|c| c.removed)
        && emulators
            .iter()
            .all(|e| !matches!(e.action, EmulatorAction::Failed | EmulatorAction::NoSdk))
        && appium.as_ref().is_none_or(|a| a.stopped);
    let summary = CleanupRunSummary {
        run_id: run_id.to_owned(),
        run_dir_found,
        cleanup_report_found,
        unresolved_orders,
        containers,
        emulators,
        appium,
        device_cleanup,
        clean,
    };
    if run_dir_found {
        write_summary(&run_dir, &summary);
    }
    info!(
        run = run_id,
        clean = summary.clean,
        "cleanup command finished"
    );
    summary
}

/// Unresolved order ids from the run's `cleanup.json`, if it exists.
fn read_unresolved(run_dir: &Path) -> (bool, Vec<String>) {
    let path = run_dir.join(files::CLEANUP);
    let Ok(text) = std::fs::read_to_string(&path) else {
        return (false, Vec::new());
    };
    match serde_json::from_str::<CleanupReport>(&text) {
        Ok(report) => (true, report.unresolved),
        Err(e) => {
            warn!(path = %path.display(), error = %e, "cleanup.json is unreadable");
            (true, vec![format!("<unreadable {}: {e}>", files::CLEANUP)])
        }
    }
}

async fn remove_containers(
    docker: &dyn ContainerRuntime,
    config: &Config,
    run_id: &str,
) -> Vec<ContainerItem> {
    let name = format!("{}-{run_id}", config.relay.container_name_prefix);
    let names = match docker.list_names(&name).await {
        Ok(names) => names,
        Err(e) => {
            return vec![ContainerItem {
                name,
                removed: false,
                error: Some(format!("cannot list containers: {e}")),
            }];
        }
    };
    // The relay is created with exactly this name; a prefix match could
    // reach another run whose id happens to extend this one.
    let mut items = Vec::with_capacity(names.len());
    for name in names.into_iter().filter(|n| *n == name) {
        let result = docker.stop_and_remove(&name).await;
        items.push(ContainerItem {
            name,
            removed: result.is_ok(),
            error: result.err().map(|e| e.to_string()),
        });
    }
    items
}

/// The command that stops an Appium server Mortsom started: it matches the
/// very argument list [`AppiumServer::start`] uses, so nothing else on the
/// host is a candidate.
fn appium_kill_command(port: u16) -> mortsom_android_driver::process::Command {
    mortsom_android_driver::process::Command::new(
        std::path::Path::new("pkill"),
        &["-f", &format!("appium --address 127.0.0.1 --port {port}")],
    )
}

/// Stops an Appium server left listening on `port`, if one is there.
///
/// Returns `None` when the port is quiet, which is the normal case: only a
/// run that died before teardown leaves one.
async fn stop_orphan_appium(runner: &dyn CommandRunner, port: u16) -> Option<AppiumItem> {
    let base = format!("http://127.0.0.1:{port}");
    if !server_ready(&base, APPIUM_PROBE).await {
        return None;
    }
    let killed = run_ok(runner, &appium_kill_command(port), APPIUM_KILL_TIMEOUT).await;
    let still_there = server_ready(&base, APPIUM_PROBE).await;
    Some(AppiumItem {
        port,
        stopped: !still_there,
        error: match (killed, still_there) {
            (_, false) => None,
            (Err(e), true) => Some(e.to_string()),
            (Ok(_), true) => Some("still answering after being asked to stop".to_owned()),
        },
    })
}

async fn shut_down_emulators(deps: &CleanupDeps<'_>, config: &Config) -> Vec<EmulatorItem> {
    let Some(android) = &config.android else {
        return Vec::new();
    };
    let sdk = match AndroidSdk::locate(deps.env, |p| p.exists()) {
        Ok(sdk) => sdk,
        Err(e) => {
            return Actor::ALL
                .into_iter()
                .map(|actor| {
                    let slot = DeviceSlot::for_actor(android, actor);
                    EmulatorItem {
                        actor,
                        serial: slot.serial,
                        avd_name: None,
                        action: EmulatorAction::NoSdk,
                        error: Some(e.to_string()),
                    }
                })
                .collect();
        }
    };
    let mut items = Vec::with_capacity(Actor::ALL.len());
    for actor in Actor::ALL {
        let slot = DeviceSlot::for_actor(android, actor);
        items.push(shut_down_slot(deps.runner, sdk.adb(), &slot).await);
    }
    items
}

/// Kills the emulator on `slot` only when it is Mortsom's AVD for that slot.
async fn shut_down_slot(
    runner: &dyn CommandRunner,
    adb_path: PathBuf,
    slot: &DeviceSlot,
) -> EmulatorItem {
    let adb = Adb::new(adb_path, slot.serial.clone());
    let item =
        |avd_name: Option<String>, action: EmulatorAction, error: Option<String>| EmulatorItem {
            actor: slot.actor,
            serial: slot.serial.clone(),
            avd_name,
            action,
            error,
        };
    let name = match adb.avd_name(runner).await {
        Ok(Some(name)) => name,
        Ok(None) => return item(None, EmulatorAction::NotRunning, None),
        // adb unusable: the slot was not inspected, so it is not clean.
        Err(e) => return item(None, EmulatorAction::Failed, Some(e.to_string())),
    };
    if name != slot.avd_name {
        return item(Some(name), EmulatorAction::Foreign, None);
    }
    match adb.emu_kill(runner).await {
        Ok(()) => item(Some(name), EmulatorAction::Killed, None),
        Err(e) => item(Some(name), EmulatorAction::Failed, Some(e.to_string())),
    }
}

fn write_summary(run_dir: &Path, summary: &CleanupRunSummary) {
    let path = run_dir.join(CLEANUP_COMMAND_FILE);
    match serde_json::to_string_pretty(summary) {
        Ok(json) => {
            if let Err(e) = std::fs::write(&path, json) {
                warn!(path = %path.display(), error = %e, "cannot write cleanup summary");
            }
        }
        Err(e) => warn!(error = %e, "cannot serialize cleanup summary"),
    }
}

#[cfg(test)]
mod tests;
