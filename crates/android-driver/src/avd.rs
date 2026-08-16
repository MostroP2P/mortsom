//! AVD discovery and creation with a pinned image, device profile and
//! deterministic hardware settings (spec Phase 2.1). Each actor gets its own
//! AVD name and therefore its own user-data directory.

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use thiserror::Error;
use tracing::info;

use crate::config::AndroidConfig;
use crate::process::{Command, CommandRunner, ProcessError, run_ok};
use crate::sdk::AndroidSdk;

/// Deadline for `avdmanager` operations.
pub const AVD_TIMEOUT: Duration = Duration::from_secs(120);

/// Deterministic hardware settings written to every Mortsom AVD, besides
/// the RAM size which comes from `[android].emulator_ram_mb`.
pub const HARDWARE_SETTINGS: &[(&str, &str)] = &[
    ("disk.dataPartition.size", "6G"),
    ("hw.gpu.enabled", "yes"),
    ("hw.gpu.mode", "swiftshader_indirect"),
    ("hw.keyboard", "yes"),
    ("hw.lcd.density", "420"),
    ("hw.audioInput", "no"),
    ("hw.audioOutput", "no"),
    ("fastboot.forceColdBoot", "no"),
    ("hw.camera.back", "none"),
    ("hw.camera.front", "none"),
];

/// Actor slots in a device farm of two.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Actor {
    /// Alice.
    Alice,
    /// Bob.
    Bob,
}

impl Actor {
    /// Both actors, in order.
    pub const ALL: [Actor; 2] = [Actor::Alice, Actor::Bob];

    /// Stable lowercase name.
    pub fn as_str(self) -> &'static str {
        match self {
            Actor::Alice => "alice",
            Actor::Bob => "bob",
        }
    }

    /// Zero-based slot index.
    pub fn index(self) -> u16 {
        match self {
            Actor::Alice => 0,
            Actor::Bob => 1,
        }
    }
}

impl std::fmt::Display for Actor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A resolved AVD definition for one actor.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AvdSpec {
    /// Actor the AVD belongs to.
    pub actor: Actor,
    /// AVD name (`<prefix>-<actor>`).
    pub name: String,
    /// System image package.
    pub image: String,
    /// Device profile id (e.g. `pixel_6`).
    pub device_profile: String,
    /// Snapshot name restored between scenarios.
    pub snapshot: String,
    /// RAM in megabytes.
    pub ram_mb: u32,
}

impl AvdSpec {
    /// Builds the spec for `actor` from configuration.
    pub fn for_actor(config: &AndroidConfig, actor: Actor) -> Self {
        Self {
            actor,
            name: format!("{}-{actor}", config.avd_name_prefix),
            image: config.avd_image.clone(),
            device_profile: config.device_profile.clone(),
            snapshot: config.snapshot_name.clone(),
            ram_mb: config.emulator_ram_mb,
        }
    }

    /// The pinned `config.ini` values for this AVD.
    pub fn hardware_settings(&self) -> Vec<(String, String)> {
        let mut settings: Vec<(String, String)> = HARDWARE_SETTINGS
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect();
        settings.push(("hw.ramSize".to_owned(), self.ram_mb.to_string()));
        settings
    }

    /// Directory holding the AVD's user data (`<avd_home>/<name>.avd`).
    pub fn data_dir(&self, avd_home: &Path) -> PathBuf {
        avd_home.join(format!("{}.avd", self.name))
    }
}

/// AVD lifecycle failures.
#[derive(Debug, Error)]
pub enum AvdError {
    /// The system image is not installed.
    #[error("system image {package} is not installed; run: sdkmanager \"{package}\"")]
    ImageMissing {
        /// Package name.
        package: String,
    },
    /// A tool invocation failed.
    #[error(transparent)]
    Process(#[from] ProcessError),
    /// The AVD config could not be written.
    #[error("cannot write AVD config {path}: {source}")]
    Config {
        /// File that failed.
        path: PathBuf,
        /// I/O error.
        #[source]
        source: std::io::Error,
    },
}

/// Parses `avdmanager list avd -c` output into names. Only AVD-safe names
/// (`[A-Za-z0-9._-]`) are accepted, so warnings and progress lines that the
/// tool prints on the same stream are ignored.
pub fn parse_avd_names(output: &str) -> Vec<String> {
    output
        .lines()
        .map(str::trim)
        .filter(|l| {
            !l.is_empty()
                && !l.ends_with(':')
                && l.chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
        })
        .map(str::to_owned)
        .collect()
}

/// Lists existing AVD names.
pub async fn list_avds(
    runner: &dyn CommandRunner,
    sdk: &AndroidSdk,
) -> Result<Vec<String>, AvdError> {
    let output = run_ok(
        runner,
        &Command::new(sdk.avdmanager(), &["list", "avd", "-c"]),
        AVD_TIMEOUT,
    )
    .await?;
    Ok(parse_avd_names(&output.stdout))
}

/// Ensures the AVD exists with the pinned image and deterministic hardware
/// settings. An existing AVD is never recreated or overwritten (no
/// `--force`); its settings are re-applied so a drifted config converges.
/// `avd_home` comes from [`crate::sdk::avd_home`].
pub async fn ensure_avd(
    runner: &dyn CommandRunner,
    sdk: &AndroidSdk,
    spec: &AvdSpec,
    avd_home: &Path,
    exists: &dyn Fn(&Path) -> bool,
) -> Result<PathBuf, AvdError> {
    if !exists(&sdk.system_image_dir(&spec.image)) {
        return Err(AvdError::ImageMissing {
            package: spec.image.clone(),
        });
    }
    let existing = list_avds(runner, sdk).await?;
    if !existing.contains(&spec.name) {
        info!(avd = %spec.name, image = %spec.image, "creating AVD");
        run_ok(
            runner,
            &Command::new(
                sdk.avdmanager(),
                &[
                    "create",
                    "avd",
                    "-n",
                    &spec.name,
                    "-k",
                    &spec.image,
                    "-d",
                    &spec.device_profile,
                ],
            ),
            AVD_TIMEOUT,
        )
        .await?;
    }
    let data_dir = spec.data_dir(avd_home);
    apply_hardware_settings(&data_dir.join("config.ini"), &spec.hardware_settings())?;
    Ok(data_dir)
}

/// Rewrites `config.ini` so every pinned key holds its value, preserving
/// other keys. Creates the file if absent.
pub fn apply_hardware_settings(path: &Path, pinned: &[(String, String)]) -> Result<(), AvdError> {
    let current = std::fs::read_to_string(path).unwrap_or_default();
    let borrowed: Vec<(&str, &str)> = pinned
        .iter()
        .map(|(k, v)| (k.as_str(), v.as_str()))
        .collect();
    let merged = merge_ini(&current, &borrowed);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|source| AvdError::Config {
            path: path.to_path_buf(),
            source,
        })?;
    }
    std::fs::write(path, merged).map_err(|source| AvdError::Config {
        path: path.to_path_buf(),
        source,
    })
}

/// Pure merge of `key=value` lines: pinned keys win, others are preserved.
pub fn merge_ini(current: &str, pinned: &[(&str, &str)]) -> String {
    let mut lines: Vec<String> = current
        .lines()
        .filter(|line| {
            let key = line.split('=').next().unwrap_or("").trim();
            !pinned.iter().any(|(k, _)| *k == key)
        })
        .map(str::to_owned)
        .collect();
    for (key, value) in pinned {
        lines.push(format!("{key}={value}"));
    }
    let mut out = lines.join("\n");
    out.push('\n');
    out
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

    #[test]
    fn spec_names_are_per_actor() {
        let alice = AvdSpec::for_actor(&config(), Actor::Alice);
        let bob = AvdSpec::for_actor(&config(), Actor::Bob);
        assert_eq!(alice.name, "mortsom-alice");
        assert_eq!(bob.name, "mortsom-bob");
        assert_ne!(
            alice.data_dir(Path::new("/h")),
            bob.data_dir(Path::new("/h"))
        );
        assert_eq!(
            alice.data_dir(Path::new("/h")),
            PathBuf::from("/h/mortsom-alice.avd")
        );
    }

    #[test]
    fn parse_avd_names_ignores_noise() {
        let out = "Parsing /sdk/x\nWARNING:\nmortsom-alice\nmortsom-bob\n[=======] 100% Loading\nWarning: JAVA_HOME unset\n";
        assert_eq!(parse_avd_names(out), vec!["mortsom-alice", "mortsom-bob"]);
    }

    #[test]
    fn merge_ini_pins_values_and_keeps_others() {
        let merged = merge_ini(
            "hw.ramSize=512\navd.ini.displayname=Foo\n",
            &[("hw.ramSize", "2048")],
        );
        assert_eq!(merged, "avd.ini.displayname=Foo\nhw.ramSize=2048\n");
    }

    #[tokio::test]
    async fn ensure_avd_creates_when_missing_and_skips_when_present() {
        let dir = tempfile::tempdir().unwrap();
        let sdk = AndroidSdk::at("/sdk");
        let runner = FakeRunner::default();
        runner.script("avdmanager list avd -c", "other\n");
        runner.script("avdmanager create avd", "");
        let spec = AvdSpec::for_actor(&config(), Actor::Alice);

        let data_dir = ensure_avd(&runner, &sdk, &spec, dir.path(), &|_| true)
            .await
            .unwrap();

        assert!(runner.calls().iter().any(|c| c.contains(
            "create avd -n mortsom-alice -k system-images;android-35;google_apis;x86_64 -d pixel_6"
        )));
        let ini = std::fs::read_to_string(data_dir.join("config.ini")).unwrap();
        assert!(ini.contains("hw.ramSize=3072"));

        let runner = FakeRunner::default();
        runner.script("avdmanager list avd -c", "mortsom-alice\n");
        ensure_avd(&runner, &sdk, &spec, dir.path(), &|_| true)
            .await
            .unwrap();
        assert!(!runner.calls().iter().any(|c| c.contains("create avd")));
    }

    #[tokio::test]
    async fn ensure_avd_refuses_without_the_system_image() {
        let dir = tempfile::tempdir().unwrap();
        let runner = FakeRunner::default();
        let spec = AvdSpec::for_actor(&config(), Actor::Bob);

        let err = ensure_avd(&runner, &AndroidSdk::at("/sdk"), &spec, dir.path(), &|_| {
            false
        })
        .await
        .unwrap_err();

        assert!(matches!(err, AvdError::ImageMissing { .. }));
        assert!(runner.calls().is_empty());
    }
    /// An AVD created between the listing and the create call must survive:
    /// `--force` would overwrite it and take another run's device with it.
    #[tokio::test]
    async fn creating_an_avd_never_forces_an_overwrite() {
        let dir = tempfile::tempdir().unwrap();
        let runner = FakeRunner::default();
        runner.script("avdmanager list avd -c", "other\n");
        let spec = AvdSpec::for_actor(&config(), Actor::Alice);

        ensure_avd(&runner, &AndroidSdk::at("/sdk"), &spec, dir.path(), &|_| {
            true
        })
        .await
        .unwrap();

        let create = runner
            .calls()
            .into_iter()
            .find(|c| c.contains("create avd"))
            .unwrap();
        assert!(!create.contains("--force"), "{create}");
    }
}
