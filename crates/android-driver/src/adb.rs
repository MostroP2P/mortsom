//! Per-device `adb` operations. Every call targets one serial explicitly so
//! Alice and Bob can never be confused.

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::process::{Command, CommandRunner, Output, ProcessError, run_ok};

/// Deadline for a short adb call.
pub const ADB_TIMEOUT: Duration = Duration::from_secs(60);
/// Deadline for an APK install.
pub const INSTALL_TIMEOUT: Duration = Duration::from_secs(300);
/// Deadline for saving or loading an emulator snapshot.
pub const SNAPSHOT_TIMEOUT: Duration = Duration::from_secs(600);
/// Locale applied to every device (BCP-47).
pub const DEVICE_LOCALE: &str = "en-US";

/// Global animation scales disabled for deterministic UI automation.
pub const ANIMATION_SETTINGS: &[&str] = &[
    "window_animation_scale",
    "transition_animation_scale",
    "animator_duration_scale",
];

/// adb failures.
#[derive(Debug, Error)]
pub enum AdbError {
    /// The command failed or could not run.
    #[error(transparent)]
    Process(#[from] ProcessError),
    /// A package query returned no usable version.
    #[error("package {package} is not installed on {serial}")]
    NotInstalled {
        /// Package name.
        package: String,
        /// Device serial.
        serial: String,
    },
    /// The installed version does not match the APK Mortsom installed.
    #[error("{serial}: {package} version {installed} does not match expected {expected}")]
    VersionMismatch {
        /// Device serial.
        serial: String,
        /// Package name.
        package: String,
        /// Version found on the device.
        installed: String,
        /// Version expected.
        expected: String,
    },
}

/// Installed package identity as reported by `dumpsys package`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackageVersion {
    /// `versionName`.
    pub version_name: String,
    /// `versionCode`.
    pub version_code: String,
}

/// An `adb` client bound to one device serial.
#[derive(Debug, Clone)]
pub struct Adb {
    adb: PathBuf,
    serial: String,
}

impl Adb {
    /// Binds `adb` to `serial` (e.g. `emulator-5554`).
    pub fn new(adb: PathBuf, serial: impl Into<String>) -> Self {
        Self {
            adb,
            serial: serial.into(),
        }
    }

    /// The device serial.
    pub fn serial(&self) -> &str {
        &self.serial
    }

    fn command(&self, args: &[&str]) -> Command {
        let mut all = vec!["-s", self.serial.as_str()];
        all.extend_from_slice(args);
        Command::new(self.adb.clone(), &all)
    }

    /// Runs `adb shell <args>` and returns its output; non-zero exit is an error.
    pub async fn shell(
        &self,
        runner: &dyn CommandRunner,
        args: &[&str],
    ) -> Result<Output, AdbError> {
        let mut all = vec!["shell"];
        all.extend_from_slice(args);
        Ok(run_ok(runner, &self.command(&all), ADB_TIMEOUT).await?)
    }

    /// Reads a system property.
    pub async fn getprop(
        &self,
        runner: &dyn CommandRunner,
        name: &str,
    ) -> Result<String, AdbError> {
        Ok(self
            .shell(runner, &["getprop", name])
            .await?
            .stdout
            .trim()
            .to_owned())
    }

    /// True once Android is fully booted: `sys.boot_completed=1`,
    /// `dev.bootcomplete=1` and the boot animation service stopped. A single
    /// property is a known flaky signal on cold-booted emulators.
    pub async fn is_booted(&self, runner: &dyn CommandRunner) -> bool {
        let sys = self.getprop(runner, "sys.boot_completed").await;
        let dev = self.getprop(runner, "dev.bootcomplete").await;
        let anim = self.getprop(runner, "init.svc.bootanim").await;
        matches!(sys.as_deref(), Ok("1"))
            && matches!(dev.as_deref(), Ok("1"))
            && matches!(anim.as_deref(), Ok("stopped"))
    }

    /// Disables window, transition and animator animations.
    pub async fn disable_animations(&self, runner: &dyn CommandRunner) -> Result<(), AdbError> {
        for setting in ANIMATION_SETTINGS {
            self.shell(runner, &["settings", "put", "global", setting, "0"])
                .await?;
        }
        Ok(())
    }

    /// Sets the device time zone through the alarm service (`cmd alarm
    /// set-timezone`), which works without root on API 30+ emulators.
    pub async fn set_timezone(&self, runner: &dyn CommandRunner, tz: &str) -> Result<(), AdbError> {
        self.shell(runner, &["cmd", "alarm", "set-timezone", tz])
            .await?;
        Ok(())
    }

    /// Sets the system locale without root through the settings provider
    /// (`system_locales`); apps read it on their next resume/launch.
    pub async fn set_locale(
        &self,
        runner: &dyn CommandRunner,
        locale: &str,
    ) -> Result<(), AdbError> {
        self.shell(
            runner,
            &["settings", "put", "system", "system_locales", locale],
        )
        .await?;
        Ok(())
    }

    /// Wakes the screen and dismisses the keyguard. Emulators here have no
    /// secure lock configured, so a dismissal is expected to succeed
    /// immediately; `is_keyguard_shown` lets callers verify with a bounded wait.
    pub async fn unlock(&self, runner: &dyn CommandRunner) -> Result<(), AdbError> {
        self.shell(runner, &["input", "keyevent", "KEYCODE_WAKEUP"])
            .await?;
        self.shell(runner, &["wm", "dismiss-keyguard"]).await?;
        Ok(())
    }

    /// True while the keyguard is showing (`dumpsys window`).
    pub async fn is_keyguard_shown(&self, runner: &dyn CommandRunner) -> bool {
        self.shell(runner, &["dumpsys", "window"])
            .await
            .map(|o| {
                o.stdout.contains("mDreamingLockscreen=true")
                    || o.stdout.contains("isStatusBarKeyguard=true")
            })
            .unwrap_or(false)
    }

    /// Installs an APK, replacing any previous version and granting runtime
    /// permissions.
    pub async fn install(&self, runner: &dyn CommandRunner, apk: &Path) -> Result<(), AdbError> {
        let apk = apk.display().to_string();
        run_ok(
            runner,
            &self.command(&["install", "-r", "-g", &apk]),
            INSTALL_TIMEOUT,
        )
        .await?;
        Ok(())
    }

    /// Reads the installed version of `package`.
    pub async fn package_version(
        &self,
        runner: &dyn CommandRunner,
        package: &str,
    ) -> Result<PackageVersion, AdbError> {
        let output = self.shell(runner, &["dumpsys", "package", package]).await?;
        parse_package_version(&output.stdout).ok_or_else(|| AdbError::NotInstalled {
            package: package.to_owned(),
            serial: self.serial.clone(),
        })
    }

    /// Verifies the installed `versionName` equals `expected`.
    pub async fn verify_version(
        &self,
        runner: &dyn CommandRunner,
        package: &str,
        expected: &str,
    ) -> Result<PackageVersion, AdbError> {
        let version = self.package_version(runner, package).await?;
        if version.version_name != expected {
            return Err(AdbError::VersionMismatch {
                serial: self.serial.clone(),
                package: package.to_owned(),
                installed: version.version_name,
                expected: expected.to_owned(),
            });
        }
        Ok(version)
    }

    /// Force-stops the app.
    pub async fn force_stop(
        &self,
        runner: &dyn CommandRunner,
        package: &str,
    ) -> Result<(), AdbError> {
        self.shell(runner, &["am", "force-stop", package]).await?;
        Ok(())
    }

    /// Clears the app's data (`pm clear`).
    pub async fn clear_data(
        &self,
        runner: &dyn CommandRunner,
        package: &str,
    ) -> Result<(), AdbError> {
        self.shell(runner, &["pm", "clear", package]).await?;
        Ok(())
    }

    /// Dumps the current logcat buffer.
    pub async fn logcat_dump(&self, runner: &dyn CommandRunner) -> Result<String, AdbError> {
        Ok(run_ok(
            runner,
            &self.command(&["logcat", "-d", "-v", "threadtime"]),
            ADB_TIMEOUT,
        )
        .await?
        .stdout)
    }

    /// Asks the emulator console to shut down (`adb emu kill`).
    pub async fn emu_kill(&self, runner: &dyn CommandRunner) -> Result<(), AdbError> {
        run_ok(runner, &self.command(&["emu", "kill"]), ADB_TIMEOUT).await?;
        Ok(())
    }

    /// Saves an emulator snapshot.
    pub async fn snapshot_save(
        &self,
        runner: &dyn CommandRunner,
        name: &str,
    ) -> Result<(), AdbError> {
        run_ok(
            runner,
            &self.command(&["emu", "avd", "snapshot", "save", name]),
            INSTALL_TIMEOUT,
        )
        .await?;
        Ok(())
    }

    /// Loads an emulator snapshot.
    pub async fn snapshot_load(
        &self,
        runner: &dyn CommandRunner,
        name: &str,
    ) -> Result<(), AdbError> {
        run_ok(
            runner,
            &self.command(&["emu", "avd", "snapshot", "load", name]),
            INSTALL_TIMEOUT,
        )
        .await?;
        Ok(())
    }
}

/// Extracts `versionName`/`versionCode` from `dumpsys package` output.
pub fn parse_package_version(dumpsys: &str) -> Option<PackageVersion> {
    let mut name = None;
    let mut code = None;
    for line in dumpsys.lines() {
        let line = line.trim();
        if let Some(v) = line.strip_prefix("versionName=") {
            name.get_or_insert(v.trim().to_owned());
        } else if let Some(rest) = line.strip_prefix("versionCode=") {
            code.get_or_insert(rest.split_whitespace().next().unwrap_or("").to_owned());
        }
    }
    Some(PackageVersion {
        version_name: name?,
        version_code: code.unwrap_or_default(),
    })
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::process::tests_support::FakeRunner;

    fn adb() -> Adb {
        Adb::new(PathBuf::from("/sdk/platform-tools/adb"), "emulator-5556")
    }

    #[test]
    fn parses_dumpsys_versions() {
        let out = "Packages:\n  Package [network.mostro.app]\n    versionCode=1077 minSdk=23 targetSdk=35\n    versionName=1.3.2\n";
        assert_eq!(
            parse_package_version(out).unwrap(),
            PackageVersion {
                version_name: "1.3.2".into(),
                version_code: "1077".into()
            }
        );
        assert!(parse_package_version("Unable to find package").is_none());
    }

    #[tokio::test]
    async fn every_call_targets_the_serial() {
        let runner = FakeRunner::default();
        runner.script(
            "adb -s emulator-5556 shell getprop sys.boot_completed",
            "1\n",
        );
        runner.script("adb -s emulator-5556 shell getprop dev.bootcomplete", "1\n");
        runner.script(
            "adb -s emulator-5556 shell getprop init.svc.bootanim",
            "stopped\n",
        );
        let adb = adb();

        assert!(adb.is_booted(&runner).await);
        adb.set_locale(&runner, "en-US").await.unwrap();
        assert!(!adb.is_keyguard_shown(&runner).await);
        adb.disable_animations(&runner).await.unwrap();
        adb.unlock(&runner).await.unwrap();
        adb.set_timezone(&runner, "UTC").await.unwrap();
        adb.install(&runner, Path::new("/tmp/app.apk"))
            .await
            .unwrap();
        adb.force_stop(&runner, "network.mostro.app").await.unwrap();
        adb.clear_data(&runner, "network.mostro.app").await.unwrap();
        adb.emu_kill(&runner).await.unwrap();
        adb.snapshot_save(&runner, "clean").await.unwrap();

        let calls = runner.calls();
        assert!(
            calls.iter().all(|c| c.starts_with("adb -s emulator-5556 ")),
            "{calls:?}"
        );
        assert!(
            calls
                .iter()
                .any(|c| c.contains("settings put global window_animation_scale 0"))
        );
        assert!(
            calls
                .iter()
                .any(|c| c.contains("install -r -g /tmp/app.apk"))
        );
        assert!(
            calls
                .iter()
                .any(|c| c.contains("pm clear network.mostro.app"))
        );
        assert!(
            calls
                .iter()
                .any(|c| c.contains("emu avd snapshot save clean"))
        );
    }

    #[tokio::test]
    async fn version_verification_reports_mismatch_and_missing() {
        let runner = FakeRunner::default();
        runner.script(
            "adb -s emulator-5556 shell dumpsys package network.mostro.app",
            "    versionCode=1077\n    versionName=1.3.2\n",
        );
        let adb = adb();

        let ok = adb
            .verify_version(&runner, "network.mostro.app", "1.3.2")
            .await
            .unwrap();
        assert_eq!(ok.version_code, "1077");
        let err = adb
            .verify_version(&runner, "network.mostro.app", "9.9.9")
            .await
            .unwrap_err();
        assert!(matches!(err, AdbError::VersionMismatch { .. }));

        runner.script(
            "adb -s emulator-5556 shell dumpsys package other",
            "Unable to find",
        );
        let missing = adb.package_version(&runner, "other").await.unwrap_err();
        assert!(matches!(missing, AdbError::NotInstalled { .. }));
    }

    #[tokio::test]
    async fn failed_shell_commands_surface_as_errors() {
        let runner = FakeRunner::default();
        runner.script_failure("adb -s emulator-5556 shell pm clear", "Failed");
        let err = adb().clear_data(&runner, "x").await.unwrap_err();
        assert!(matches!(
            err,
            AdbError::Process(ProcessError::Failed { .. })
        ));
        assert!(
            !adb().is_booted(&FakeRunner::default()).await,
            "empty getprop is not booted"
        );
        let half = FakeRunner::default();
        half.script(
            "adb -s emulator-5556 shell getprop sys.boot_completed",
            "1\n",
        );
        half.script("adb -s emulator-5556 shell getprop dev.bootcomplete", "1\n");
        half.script(
            "adb -s emulator-5556 shell getprop init.svc.bootanim",
            "running\n",
        );
        assert!(
            !adb().is_booted(&half).await,
            "boot animation still running"
        );
    }
}
