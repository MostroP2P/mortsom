//! Emulator lifecycle for one actor (spec §16.5): distinct AVD, console and
//! adb ports, boot readiness, deterministic device configuration, APK
//! installation with version verification, snapshots and shutdown. Killing
//! one actor's emulator can never affect the other: every operation is bound
//! to that actor's serial and process handle.

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use thiserror::Error;
use tracing::{info, warn};

use crate::adb::{Adb, AdbError, DEVICE_LOCALE, PackageVersion};
use crate::avd::{Actor, AvdSpec};
use crate::config::AndroidConfig;
use crate::process::{ChildProcess, Command, CommandRunner, ProcessError};
use crate::sdk::AndroidSdk;

/// Poll interval while waiting for boot.
pub const BOOT_POLL: Duration = Duration::from_secs(2);
/// How long a graceful `emu kill` may take before the process is killed.
pub const GRACEFUL_SHUTDOWN: Duration = Duration::from_secs(20);
/// Time zone applied to every device.
pub const DEVICE_TIMEZONE: &str = "UTC";

/// Emulator failures.
#[derive(Debug, Error)]
pub enum EmulatorError {
    /// The emulator process could not be started.
    #[error(transparent)]
    Process(#[from] ProcessError),
    /// adb failed.
    #[error(transparent)]
    Adb(#[from] AdbError),
    /// The device did not finish booting in time.
    #[error("{serial} did not boot within {timeout:?}")]
    BootTimeout {
        /// Device serial.
        serial: String,
        /// Deadline used.
        timeout: Duration,
    },
    /// The emulator process exited before boot completed.
    #[error("emulator {serial} exited during boot: {reason} (full log: {log})")]
    Exited {
        /// Device serial.
        serial: String,
        /// Log path.
        log: PathBuf,
        /// The end of that log, where the reason usually is.
        reason: String,
    },
}

/// How many trailing log lines an exit failure carries.
const LOG_TAIL_LINES: usize = 3;

/// The end of an emulator log, joined into one line. An emulator that dies
/// during boot explains itself there -- a missing KVM device, a window
/// server that dropped it -- and the operator should read that in the
/// failure instead of opening a file to find it.
pub fn last_log_lines(log: &Path) -> String {
    let Ok(text) = std::fs::read_to_string(log) else {
        return "the emulator log could not be read".to_owned();
    };
    let tail: Vec<&str> = text
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .rev()
        .take(LOG_TAIL_LINES)
        .collect();
    if tail.is_empty() {
        return "the emulator log is empty".to_owned();
    }
    tail.into_iter().rev().collect::<Vec<_>>().join(" | ")
}

/// Ports and identity of one actor's emulator.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceSlot {
    /// Actor.
    pub actor: Actor,
    /// AVD name.
    pub avd_name: String,
    /// Emulator console port.
    pub console_port: u16,
    /// adb port (`console_port + 1`).
    pub adb_port: u16,
    /// adb serial (`emulator-<console_port>`).
    pub serial: String,
}

impl DeviceSlot {
    /// Computes the slot for `actor` from configuration.
    pub fn for_actor(config: &AndroidConfig, actor: Actor) -> Self {
        let console_port = config.console_port(actor.index());
        Self {
            actor,
            avd_name: AvdSpec::for_actor(config, actor).name,
            console_port,
            adb_port: console_port + 1,
            serial: format!("emulator-{console_port}"),
        }
    }
}

/// Emulator launch options.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaunchOptions {
    /// Run without a window.
    pub headless: bool,
    /// Boot from this snapshot when it exists; otherwise cold boot.
    pub snapshot: Option<String>,
    /// Wipe user data before boot (baseline creation).
    pub wipe_data: bool,
}

impl Default for LaunchOptions {
    fn default() -> Self {
        Self {
            headless: true,
            snapshot: None,
            wipe_data: false,
        }
    }
}

/// Builds the `emulator` command line for a slot. Pure and unit tested.
/// RAM is passed explicitly (`-memory`) so the run configuration, not a
/// drifted `config.ini`, is authoritative.
pub fn emulator_command(
    sdk: &AndroidSdk,
    slot: &DeviceSlot,
    ram_mb: u32,
    options: &LaunchOptions,
) -> Command {
    let port = slot.console_port.to_string();
    let memory = ram_mb.to_string();
    let mut args: Vec<&str> = vec![
        "-avd",
        &slot.avd_name,
        "-port",
        &port,
        "-memory",
        &memory,
        "-no-boot-anim",
        "-no-audio",
        "-gpu",
        "swiftshader_indirect",
        "-netdelay",
        "none",
        "-netspeed",
        "full",
    ];
    if options.headless {
        args.push("-no-window");
    }
    if options.wipe_data {
        args.push("-wipe-data");
    }
    let snapshot_args;
    match &options.snapshot {
        Some(name) => {
            snapshot_args = ["-snapshot", name.as_str(), "-no-snapshot-save"];
            args.extend_from_slice(&snapshot_args);
        }
        None => args.push("-no-snapshot-load"),
    }
    Command::new(sdk.emulator(), &args)
}

/// A booted emulator owned by this run.
pub struct Emulator {
    slot: DeviceSlot,
    adb: Adb,
    process: Box<dyn ChildProcess>,
    log: PathBuf,
}

impl std::fmt::Debug for Emulator {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Emulator")
            .field("slot", &self.slot)
            .field("log", &self.log)
            .finish_non_exhaustive()
    }
}

impl Emulator {
    /// Starts the emulator for `slot`, waits for boot and applies the
    /// deterministic device configuration.
    pub async fn boot(
        runner: &dyn CommandRunner,
        sdk: &AndroidSdk,
        slot: DeviceSlot,
        ram_mb: u32,
        options: &LaunchOptions,
        log_dir: &Path,
        boot_timeout: Duration,
    ) -> Result<Self, EmulatorError> {
        let log = log_dir.join(format!("emulator-{}.log", slot.actor));
        let command = emulator_command(sdk, &slot, ram_mb, options);
        info!(actor = %slot.actor, serial = %slot.serial, "starting emulator");
        let process = runner.spawn(&command, Some(&log)).await?;
        let adb = Adb::new(sdk.adb(), slot.serial.clone());
        let emulator = Self {
            slot,
            adb,
            process,
            log,
        };
        // A failed startup must not leave the emulator running: the next
        // run would find a stale device on this serial.
        if let Err(e) = emulator.wait_for_boot(runner, boot_timeout).await {
            emulator.kill_after_failed_start().await;
            return Err(e);
        }
        if let Err(e) = emulator.configure(runner).await {
            emulator.kill_after_failed_start().await;
            return Err(e);
        }
        Ok(emulator)
    }

    /// Best effort teardown of a process that never became usable; the
    /// startup error is the one worth reporting.
    async fn kill_after_failed_start(&self) {
        if let Err(e) = self.process.kill().await {
            warn!(actor = %self.slot.actor, error = %e, "cannot kill the emulator after a failed start");
        }
    }

    async fn wait_for_boot(
        &self,
        runner: &dyn CommandRunner,
        timeout: Duration,
    ) -> Result<(), EmulatorError> {
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            if self.adb.is_booted(runner).await {
                info!(serial = %self.slot.serial, "boot completed");
                return Ok(());
            }
            if !self.process.is_alive().await {
                return Err(EmulatorError::Exited {
                    serial: self.slot.serial.clone(),
                    log: self.log.clone(),
                    reason: last_log_lines(&self.log),
                });
            }
            if tokio::time::Instant::now() >= deadline {
                return Err(EmulatorError::BootTimeout {
                    serial: self.slot.serial.clone(),
                    timeout,
                });
            }
            tokio::time::sleep(BOOT_POLL).await;
        }
    }

    /// Disables animations, pins locale and time zone, and unlocks the
    /// screen. Network host mapping needs no configuration: the emulator
    /// reaches host services on `10.0.2.2`. Locale and time zone are
    /// best-effort: system images differ in what unprivileged adb may set,
    /// and neither changes what a scenario asserts, so a refusal is logged
    /// rather than failing the boot.
    async fn configure(&self, runner: &dyn CommandRunner) -> Result<(), EmulatorError> {
        self.adb.disable_animations(runner).await?;
        if let Err(e) = self.adb.set_locale(runner, DEVICE_LOCALE).await {
            warn!(serial = %self.slot.serial, error = %e, "cannot set locale; continuing");
        }
        if let Err(e) = self.adb.set_timezone(runner, DEVICE_TIMEZONE).await {
            warn!(serial = %self.slot.serial, error = %e, "cannot set time zone; continuing");
        }
        self.adb.unlock(runner).await?;
        Ok(())
    }

    /// The slot this emulator occupies.
    pub fn slot(&self) -> &DeviceSlot {
        &self.slot
    }

    /// The bound adb client.
    pub fn adb(&self) -> &Adb {
        &self.adb
    }

    /// Emulator process log path.
    pub fn log_path(&self) -> &Path {
        &self.log
    }

    /// Installs `apk` and verifies the installed `package` reports
    /// `expected_version`, so both devices are proven to run the same build.
    pub async fn install_and_verify(
        &self,
        runner: &dyn CommandRunner,
        apk: &Path,
        package: &str,
        expected_version: &str,
    ) -> Result<PackageVersion, EmulatorError> {
        self.adb.install(runner, apk).await?;
        Ok(self
            .adb
            .verify_version(runner, package, expected_version)
            .await?)
    }

    /// Saves the clean snapshot after baseline boot.
    pub async fn save_snapshot(
        &self,
        runner: &dyn CommandRunner,
        name: &str,
    ) -> Result<(), EmulatorError> {
        Ok(self.adb.snapshot_save(runner, name).await?)
    }

    /// Restores the clean snapshot between scenarios.
    pub async fn restore_snapshot(
        &self,
        runner: &dyn CommandRunner,
        name: &str,
    ) -> Result<(), EmulatorError> {
        Ok(self.adb.snapshot_load(runner, name).await?)
    }

    /// Whether the emulator process is still alive.
    pub async fn is_alive(&self) -> bool {
        self.process.is_alive().await
    }

    /// Shuts the emulator down: graceful console kill, a bounded wait for
    /// the process to exit (so snapshot state is flushed), then a hard kill
    /// as fallback. Only this actor's serial and process are touched.
    pub async fn shutdown(&self, runner: &dyn CommandRunner) -> Result<(), EmulatorError> {
        self.shutdown_with(runner, GRACEFUL_SHUTDOWN).await
    }

    /// [`Emulator::shutdown`] with an explicit grace period.
    pub async fn shutdown_with(
        &self,
        runner: &dyn CommandRunner,
        grace: Duration,
    ) -> Result<(), EmulatorError> {
        match self.adb.emu_kill(runner).await {
            Ok(()) => {
                let deadline = tokio::time::Instant::now() + grace;
                while self.process.is_alive().await && tokio::time::Instant::now() < deadline {
                    tokio::time::sleep(Duration::from_millis(250)).await;
                }
            }
            Err(e) => {
                warn!(serial = %self.slot.serial, error = %e, "adb emu kill failed; killing process")
            }
        }
        self.process.kill().await?;
        Ok(())
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::process::tests_support::FakeRunner;

    fn config() -> AndroidConfig {
        AndroidConfig {
            avd_image: "system-images;android-35;google_apis;x86_64".into(),
            device_profile: "pixel_6".into(),
            api_level: 35,
            boot_timeout_secs: 180,
            ui_timeout_secs: 30,
            snapshot_name: "mortsom-clean".into(),
            avd_name_prefix: "mortsom".into(),
            base_console_port: 5554,
            emulator_ram_mb: 3072,
            headless: true,
            appium_port: 4723,
            base_system_port: 8200,
        }
    }

    fn script_booted(runner: &FakeRunner, serial: &str) {
        runner.script(
            &format!("adb -s {serial} shell getprop sys.boot_completed"),
            "1\n",
        );
        runner.script(
            &format!("adb -s {serial} shell getprop dev.bootcomplete"),
            "1\n",
        );
        runner.script(
            &format!("adb -s {serial} shell getprop init.svc.bootanim"),
            "stopped\n",
        );
    }

    #[test]
    fn slots_are_disjoint_per_actor() {
        let alice = DeviceSlot::for_actor(&config(), Actor::Alice);
        let bob = DeviceSlot::for_actor(&config(), Actor::Bob);
        assert_eq!(
            (alice.console_port, alice.adb_port, alice.serial.as_str()),
            (5554, 5555, "emulator-5554")
        );
        assert_eq!(
            (bob.console_port, bob.adb_port, bob.serial.as_str()),
            (5556, 5557, "emulator-5556")
        );
        assert_ne!(alice.avd_name, bob.avd_name);
    }

    #[test]
    fn emulator_command_is_deterministic() {
        let slot = DeviceSlot::for_actor(&config(), Actor::Bob);
        let cmd = emulator_command(
            &AndroidSdk::at("/sdk"),
            &slot,
            3072,
            &LaunchOptions::default(),
        );
        assert_eq!(cmd.program, PathBuf::from("/sdk/emulator/emulator"));
        assert!(cmd.args.windows(2).any(|w| w == ["-avd", "mortsom-bob"]));
        assert!(cmd.args.windows(2).any(|w| w == ["-port", "5556"]));
        assert!(cmd.args.windows(2).any(|w| w == ["-memory", "3072"]));
        assert!(cmd.args.contains(&"-no-window".to_owned()));
        assert!(cmd.args.contains(&"-no-snapshot-load".to_owned()));

        let with_snapshot = emulator_command(
            &AndroidSdk::at("/sdk"),
            &slot,
            2048,
            &LaunchOptions {
                headless: false,
                snapshot: Some("clean".into()),
                wipe_data: true,
            },
        );
        assert!(!with_snapshot.args.contains(&"-no-window".to_owned()));
        assert!(with_snapshot.args.contains(&"-wipe-data".to_owned()));
        assert!(
            with_snapshot
                .args
                .windows(2)
                .any(|w| w == ["-snapshot", "clean"])
        );
    }

    #[tokio::test]
    async fn boot_waits_configures_installs_and_shuts_down_only_its_serial() {
        let dir = tempfile::tempdir().unwrap();
        let runner = FakeRunner::default();
        script_booted(&runner, "emulator-5554");
        runner.script(
            "adb -s emulator-5554 shell dumpsys package network.mostro.app",
            "versionName=1.3.2\nversionCode=1077\n",
        );
        let slot = DeviceSlot::for_actor(&config(), Actor::Alice);

        let emulator = Emulator::boot(
            &runner,
            &AndroidSdk::at("/sdk"),
            slot,
            3072,
            &LaunchOptions::default(),
            dir.path(),
            Duration::from_secs(5),
        )
        .await
        .unwrap();
        let version = emulator
            .install_and_verify(
                &runner,
                Path::new("/tmp/app.apk"),
                "network.mostro.app",
                "1.3.2",
            )
            .await
            .unwrap();
        emulator
            .save_snapshot(&runner, "mortsom-clean")
            .await
            .unwrap();
        emulator
            .shutdown_with(&runner, Duration::from_millis(10))
            .await
            .unwrap();

        assert_eq!(version.version_name, "1.3.2");
        assert!(!emulator.is_alive().await);
        let calls = runner.calls();
        assert!(
            calls.iter().all(|c| c.contains("-s emulator-5554")),
            "{calls:?}"
        );
        assert!(
            calls
                .iter()
                .any(|c| c.contains("settings put global animator_duration_scale 0"))
        );
        assert!(
            calls
                .iter()
                .any(|c| c.contains("cmd alarm set-timezone UTC"))
        );
        assert!(calls.iter().any(|c| c.contains("wm dismiss-keyguard")));
        assert!(calls.iter().any(|c| c.contains("emu kill")));
        assert_eq!(runner.spawned().len(), 1);
        assert!(runner.spawned()[0].contains("-avd mortsom-alice"));
    }

    #[tokio::test]
    async fn killing_alice_leaves_bob_alive() {
        let dir = tempfile::tempdir().unwrap();
        let runner = FakeRunner::default();
        script_booted(&runner, "emulator-5554");
        script_booted(&runner, "emulator-5556");
        let sdk = AndroidSdk::at("/sdk");
        let alice = Emulator::boot(
            &runner,
            &sdk,
            DeviceSlot::for_actor(&config(), Actor::Alice),
            3072,
            &LaunchOptions::default(),
            dir.path(),
            Duration::from_secs(5),
        )
        .await
        .unwrap();
        let bob = Emulator::boot(
            &runner,
            &sdk,
            DeviceSlot::for_actor(&config(), Actor::Bob),
            3072,
            &LaunchOptions::default(),
            dir.path(),
            Duration::from_secs(5),
        )
        .await
        .unwrap();

        alice
            .shutdown_with(&runner, Duration::from_millis(10))
            .await
            .unwrap();

        assert!(!alice.is_alive().await);
        assert!(bob.is_alive().await);
        let kills: Vec<_> = runner
            .calls()
            .into_iter()
            .filter(|c| c.contains("emu kill"))
            .collect();
        assert_eq!(kills, vec!["adb -s emulator-5554 emu kill".to_owned()]);
        assert_ne!(alice.log_path(), bob.log_path());
    }

    #[tokio::test]
    async fn locale_and_timezone_refusals_do_not_fail_boot() {
        let dir = tempfile::tempdir().unwrap();
        let runner = FakeRunner::default();
        script_booted(&runner, "emulator-5554");
        runner.script_failure(
            "adb -s emulator-5554 shell cmd alarm set-timezone",
            "denied",
        );
        runner.script_failure(
            "adb -s emulator-5554 shell settings put system system_locales",
            "denied",
        );
        let slot = DeviceSlot::for_actor(&config(), Actor::Alice);

        let emulator = Emulator::boot(
            &runner,
            &AndroidSdk::at("/sdk"),
            slot,
            3072,
            &LaunchOptions::default(),
            dir.path(),
            Duration::from_secs(5),
        )
        .await
        .unwrap();

        assert!(emulator.is_alive().await);
    }

    #[tokio::test]
    async fn boot_timeout_is_reported() {
        let dir = tempfile::tempdir().unwrap();
        let runner = FakeRunner::default();
        runner.script(
            "adb -s emulator-5554 shell getprop sys.boot_completed",
            "0\n",
        );
        let slot = DeviceSlot::for_actor(&config(), Actor::Alice);

        let err = Emulator::boot(
            &runner,
            &AndroidSdk::at("/sdk"),
            slot,
            3072,
            &LaunchOptions::default(),
            dir.path(),
            Duration::from_millis(10),
        )
        .await
        .unwrap_err();

        assert!(matches!(err, EmulatorError::BootTimeout { .. }));
        // The emulator that never booted must not outlive the failure.
        assert_eq!(
            runner.children_alive(),
            vec![false],
            "a failed start leaked the emulator process"
        );
    }
    /// An emulator that dies during boot says why in its log, and the
    /// operator should not have to open a file to learn that the window
    /// server dropped it.
    #[test]
    fn the_boot_failure_carries_the_end_of_the_emulator_log() {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("emulator-alice.log");
        std::fs::write(
            &log,
            "INFO | booting\nINFO | Created extended window\nX connection to :1 broken\n",
        )
        .unwrap();

        let error = EmulatorError::Exited {
            serial: "emulator-5554".into(),
            log: log.clone(),
            reason: last_log_lines(&log),
        };

        let shown = error.to_string();
        assert!(shown.contains("X connection to :1 broken"), "{shown}");
        assert!(shown.contains("emulator-5554"), "{shown}");
        assert!(shown.contains("emulator-alice.log"), "{shown}");
    }
}
