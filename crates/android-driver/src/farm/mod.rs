//! Device farm of two (spec Phase 2): brings Alice and Bob up on isolated
//! AVDs, installs the exact same APK on both, starts the automation server
//! and opens one session per device; tears everything down in reverse
//! order without stopping at the first error.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use thiserror::Error;
use tracing::{error, info, warn};

use crate::adb::PackageVersion;
use crate::appium::{AppiumError, SessionSpec};
use crate::avd::{Actor, AvdError, AvdSpec, ensure_avd};
use crate::config::AndroidConfig;
use crate::emulator::{DeviceSlot, Emulator, EmulatorError, LaunchOptions};
use crate::process::CommandRunner;
use crate::sdk::AndroidSdk;
use crate::ui::{UiDriver, UiServer, UiSession};

/// Deadline for the automation server to become ready.
pub const SERVER_READY_TIMEOUT: Duration = Duration::from_secs(60);

/// What to bring up.
#[derive(Debug, Clone)]
pub struct FarmSpec {
    /// Android matrix.
    pub android: AndroidConfig,
    /// APK installed on both devices.
    pub apk: PathBuf,
    /// Application id.
    pub package: String,
    /// Launch activity, if any.
    pub main_activity: Option<String>,
    /// Where AVDs live.
    pub avd_home: PathBuf,
    /// Directory for emulator/appium logs.
    pub log_dir: PathBuf,
    /// Grace period for a graceful emulator shutdown before a hard kill.
    pub shutdown_grace: Duration,
}

/// Farm failures.
#[derive(Debug, Error)]
pub enum FarmError {
    /// AVD preparation failed.
    #[error("{actor}: {source}")]
    Avd {
        /// Actor.
        actor: Actor,
        /// Cause.
        #[source]
        source: AvdError,
    },
    /// Emulator boot, configuration or install failed.
    #[error("{actor}: {source}")]
    Emulator {
        /// Actor.
        actor: Actor,
        /// Cause.
        #[source]
        source: EmulatorError,
    },
    /// The two devices do not run the same build.
    #[error("installed versions differ: alice {alice} vs bob {bob}")]
    VersionMismatch {
        /// Alice's version.
        alice: String,
        /// Bob's version.
        bob: String,
    },
    /// Automation server or session failed.
    #[error("automation: {0}")]
    Ui(#[from] AppiumError),
}

/// One actor's live device.
pub struct Device {
    /// Actor.
    pub actor: Actor,
    /// Booted emulator.
    pub emulator: Emulator,
    /// UI session, shareable with scenario contexts.
    pub session: Arc<dyn UiSession>,
    /// Session spec used.
    pub spec: SessionSpec,
    /// Installed app version.
    pub version: PackageVersion,
}

/// Outcome of tearing one resource down.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TeardownItem {
    /// Resource name (`alice session`, `appium server`, `bob emulator`).
    pub resource: String,
    /// True when released.
    pub ok: bool,
    /// Error text when not.
    pub error: Option<String>,
}

/// The live farm.
pub struct DeviceFarm {
    devices: Vec<Device>,
    server: Box<dyn UiServer>,
    shutdown_grace: Duration,
}

impl std::fmt::Debug for DeviceFarm {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DeviceFarm")
            .field(
                "devices",
                &self.devices.iter().map(|d| d.actor).collect::<Vec<_>>(),
            )
            .finish_non_exhaustive()
    }
}

impl DeviceFarm {
    /// Brings both devices up. On any failure everything already started is
    /// shut down before the error is returned.
    pub async fn bring_up(
        runner: &dyn CommandRunner,
        ui: &dyn UiDriver,
        sdk: &AndroidSdk,
        spec: &FarmSpec,
    ) -> Result<Self, FarmError> {
        for actor in Actor::ALL {
            let avd = AvdSpec::for_actor(&spec.android, actor);
            ensure_avd(runner, sdk, &avd, &spec.avd_home, &|p: &Path| p.exists())
                .await
                .map_err(|source| FarmError::Avd { actor, source })?;
        }
        let (alice, bob) = tokio::join!(
            boot(runner, sdk, spec, Actor::Alice),
            boot(runner, sdk, spec, Actor::Bob)
        );
        let emulators = match (alice, bob) {
            (Ok(a), Ok(b)) => vec![a, b],
            (a, b) => return Err(shutdown_partial(runner, spec, [a, b]).await),
        };
        match Self::finish_bring_up(runner, ui, spec, emulators).await {
            Ok(farm) => Ok(farm),
            Err((emulators, err)) => {
                // A device that never became drivable takes the reason with
                // it when it stops, and the reason lives in the logcat --
                // which is exactly what Appium tells the operator to read.
                // This is the last moment it can be collected.
                dump_logcats(runner, spec, &emulators).await;
                for emulator in &emulators {
                    let _ = emulator.shutdown_with(runner, spec.shutdown_grace).await;
                }
                Err(*err)
            }
        }
    }

    async fn finish_bring_up(
        runner: &dyn CommandRunner,
        ui: &dyn UiDriver,
        spec: &FarmSpec,
        emulators: Vec<Emulator>,
    ) -> Result<Self, (Vec<Emulator>, Box<FarmError>)> {
        let versions = match install_all(runner, spec, &emulators).await {
            Ok(versions) => versions,
            Err(e) => return Err((emulators, Box::new(e))),
        };
        let server = match ui
            .start_server(
                runner,
                spec.android.appium_port,
                &spec.log_dir,
                SERVER_READY_TIMEOUT,
            )
            .await
        {
            Ok(server) => server,
            Err(e) => return Err((emulators, Box::new(e.into()))),
        };
        let mut devices = Vec::new();
        for (emulator, version) in emulators.into_iter().zip(versions) {
            let actor = emulator.slot().actor;
            let session_spec = SessionSpec {
                udid: emulator.slot().serial.clone(),
                system_port: spec.android.system_port(actor.index()),
                app_package: spec.package.clone(),
                app_activity: spec.main_activity.clone(),
            };
            match server.create_session(&session_spec).await {
                Ok(session) => devices.push(Device {
                    actor,
                    emulator,
                    session: Arc::from(session),
                    spec: session_spec,
                    version,
                }),
                Err(e) => {
                    let mut all: Vec<Emulator> = devices.into_iter().map(|d| d.emulator).collect();
                    all.push(emulator);
                    let _ = server.stop().await;
                    return Err((all, Box::new(e.into())));
                }
            }
        }
        info!("device farm ready: alice and bob booted, same build installed, sessions open");
        Ok(Self {
            devices,
            server,
            shutdown_grace: spec.shutdown_grace,
        })
    }

    /// The device of `actor`.
    pub fn device(&self, actor: Actor) -> &Device {
        // Both actors are always present by construction.
        self.devices
            .iter()
            .find(|d| d.actor == actor)
            .unwrap_or(&self.devices[0])
    }

    /// Both devices.
    pub fn devices(&self) -> &[Device] {
        &self.devices
    }

    /// Releases sessions, server and emulators in that order, recording
    /// every outcome and never stopping early.
    pub async fn tear_down(self, runner: &dyn CommandRunner) -> Vec<TeardownItem> {
        let mut items = Vec::new();
        for device in &self.devices {
            items.push(item(
                format!("{} session", device.actor),
                device.session.end().await,
            ));
        }
        items.push(item("appium server".to_owned(), self.server.stop().await));
        for device in &self.devices {
            items.push(item(
                format!("{} emulator", device.actor),
                device
                    .emulator
                    .shutdown_with(runner, self.shutdown_grace)
                    .await,
            ));
        }
        for failed in items.iter().filter(|i| !i.ok) {
            error!(resource = %failed.resource, error = ?failed.error, "teardown item failed");
        }
        items
    }
}

/// Installs the APK on every emulator and proves both report the same build.
async fn install_all(
    runner: &dyn CommandRunner,
    spec: &FarmSpec,
    emulators: &[Emulator],
) -> Result<Vec<PackageVersion>, FarmError> {
    let mut versions = Vec::new();
    for emulator in emulators {
        let actor = emulator.slot().actor;
        emulator
            .adb()
            .install(runner, &spec.apk)
            .await
            .map_err(|e| FarmError::Emulator {
                actor,
                source: e.into(),
            })?;
        let version = emulator
            .adb()
            .package_version(runner, &spec.package)
            .await
            .map_err(|e| FarmError::Emulator {
                actor,
                source: e.into(),
            })?;
        versions.push(version);
    }
    if let [alice, bob] = versions.as_slice()
        && alice != bob
    {
        // Two builds can share a version name and differ in code; the
        // report must show what actually differs.
        return Err(FarmError::VersionMismatch {
            alice: format!("{} ({})", alice.version_name, alice.version_code),
            bob: format!("{} ({})", bob.version_name, bob.version_code),
        });
    }
    Ok(versions)
}

/// Writes `device-logcat-<actor>.txt` for every emulator still running, next
/// to the emulator logs. Best effort: a failure to collect evidence must not
/// replace the failure being explained.
async fn dump_logcats(runner: &dyn CommandRunner, spec: &FarmSpec, emulators: &[Emulator]) {
    for emulator in emulators {
        let actor = emulator.slot().actor;
        let path = spec.log_dir.join(format!("device-logcat-{actor}.txt"));
        match emulator.adb().logcat_dump(runner).await {
            Ok(logcat) => {
                if let Err(e) = std::fs::write(&path, logcat) {
                    warn!(actor = %actor, error = %e, "cannot write the logcat of a device that failed");
                }
            }
            Err(e) => {
                warn!(actor = %actor, error = %e, "cannot dump the logcat of a device that failed")
            }
        }
    }
}

/// Shuts down whichever emulators booted and returns the first boot error.
async fn shutdown_partial(
    runner: &dyn CommandRunner,
    spec: &FarmSpec,
    results: [Result<Emulator, FarmError>; 2],
) -> FarmError {
    let mut first_error = None;
    for result in results {
        match result {
            Ok(booted) => {
                let _ = booted.shutdown_with(runner, spec.shutdown_grace).await;
            }
            Err(e) => {
                first_error.get_or_insert(e);
            }
        }
    }
    // The caller only reaches this path when at least one boot failed; the
    // fallback keeps the function total without panicking.
    first_error.unwrap_or_else(|| {
        FarmError::Ui(AppiumError::NotReady {
            url: "internal: shutdown_partial called without a boot failure".into(),
            timeout: Duration::ZERO,
        })
    })
}

fn item<E: std::fmt::Display>(resource: String, result: Result<(), E>) -> TeardownItem {
    TeardownItem {
        resource,
        ok: result.is_ok(),
        error: result.err().map(|e| e.to_string()),
    }
}

async fn boot(
    runner: &dyn CommandRunner,
    sdk: &AndroidSdk,
    spec: &FarmSpec,
    actor: Actor,
) -> Result<Emulator, FarmError> {
    let options = LaunchOptions {
        headless: spec.android.headless,
        snapshot: None,
        wipe_data: false,
    };
    Emulator::boot(
        runner,
        sdk,
        DeviceSlot::for_actor(&spec.android, actor),
        spec.android.emulator_ram_mb,
        &options,
        &spec.log_dir,
        Duration::from_secs(spec.android.boot_timeout_secs),
    )
    .await
    .map_err(|source| FarmError::Emulator { actor, source })
}

#[cfg(test)]
mod tests;
