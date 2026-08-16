#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::{Arc, Mutex};

use async_trait::async_trait;

use super::*;
use crate::process::tests_support::FakeRunner;

/// Fake UI stack: records session specs and lets tests fail creation.
#[derive(Default, Clone)]
struct FakeUi {
    sessions: Arc<Mutex<Vec<SessionSpec>>>,
    ended: Arc<Mutex<Vec<String>>>,
    fail_session_for: Option<String>,
    fail_server: bool,
}

struct FakeServer(FakeUi);
struct FakeSession(FakeUi, String);

#[async_trait]
impl UiDriver for FakeUi {
    async fn start_server(
        &self,
        _runner: &dyn CommandRunner,
        _port: u16,
        _log_dir: &Path,
        _ready_timeout: Duration,
    ) -> Result<Box<dyn UiServer>, AppiumError> {
        if self.fail_server {
            return Err(AppiumError::NotReady {
                url: "x".into(),
                timeout: Duration::ZERO,
            });
        }
        Ok(Box::new(FakeServer(self.clone())))
    }
}

#[async_trait]
impl UiServer for FakeServer {
    async fn create_session(&self, spec: &SessionSpec) -> Result<Box<dyn UiSession>, AppiumError> {
        if self.0.fail_session_for.as_deref() == Some(spec.udid.as_str()) {
            return Err(AppiumError::NotReady {
                url: spec.udid.clone(),
                timeout: Duration::ZERO,
            });
        }
        self.0.sessions.lock().unwrap().push(spec.clone());
        Ok(Box::new(FakeSession(self.0.clone(), spec.udid.clone())))
    }
    async fn stop(&self) -> Result<(), AppiumError> {
        Ok(())
    }
}

#[async_trait]
impl UiSession for FakeSession {
    async fn exists(&self, _: &str) -> Result<bool, AppiumError> {
        Ok(true)
    }
    async fn wait_for(&self, _: &str, _: Duration) -> Result<(), AppiumError> {
        Ok(())
    }
    async fn wait_gone(&self, _: &str, _: Duration) -> Result<(), AppiumError> {
        Ok(())
    }
    async fn tap(&self, _: &str) -> Result<(), AppiumError> {
        Ok(())
    }
    async fn type_text(&self, _: &str, _: &str) -> Result<(), AppiumError> {
        Ok(())
    }
    async fn text_of(&self, _: &str) -> Result<String, AppiumError> {
        Ok(String::new())
    }
    async fn is_enabled(&self, _: &str) -> Result<bool, AppiumError> {
        Ok(true)
    }
    async fn label_of(&self, _: &str) -> Result<String, AppiumError> {
        Ok(String::new())
    }
    async fn screenshot_png(&self) -> Result<Vec<u8>, AppiumError> {
        Ok(vec![1])
    }
    async fn page_source(&self) -> Result<String, AppiumError> {
        Ok("<hierarchy/>".into())
    }
    async fn background(&self, _: u64) -> Result<(), AppiumError> {
        Ok(())
    }
    async fn terminate_app(&self) -> Result<(), AppiumError> {
        Ok(())
    }
    async fn activate_app(&self) -> Result<(), AppiumError> {
        Ok(())
    }
    async fn clear_app(&self) -> Result<(), AppiumError> {
        Ok(())
    }
    async fn set_connectivity(&self, _: bool) -> Result<(), AppiumError> {
        Ok(())
    }
    async fn set_orientation(&self, _: bool) -> Result<(), AppiumError> {
        Ok(())
    }
    async fn end(&self) -> Result<(), AppiumError> {
        self.0.ended.lock().unwrap().push(self.1.clone());
        Ok(())
    }
}

fn config() -> AndroidConfig {
    AndroidConfig {
        avd_image: "system-images;android-35;google_apis;x86_64".into(),
        device_profile: "pixel_6".into(),
        api_level: 35,
        boot_timeout_secs: 5,
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

fn spec(dir: &Path) -> FarmSpec {
    FarmSpec {
        android: config(),
        apk: dir.join("app.apk"),
        package: "network.mostro.app".into(),
        main_activity: None,
        avd_home: dir.join("avd"),
        log_dir: dir.to_path_buf(),
        shutdown_grace: Duration::from_millis(10),
    }
}

/// SDK rooted in a temp dir with the system image directory present.
fn sdk(dir: &Path) -> AndroidSdk {
    let root = dir.join("sdk");
    std::fs::create_dir_all(root.join("system-images/android-35/google_apis/x86_64")).unwrap();
    AndroidSdk::at(root)
}

fn healthy_runner(version_bob: &str) -> FakeRunner {
    let runner = FakeRunner::default();
    runner.script("avdmanager list avd -c", "mortsom-alice\nmortsom-bob\n");
    for serial in ["emulator-5554", "emulator-5556"] {
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
    runner.script(
        "adb -s emulator-5554 shell dumpsys package network.mostro.app",
        "versionName=1.3.2\nversionCode=1077\n",
    );
    runner.script(
        "adb -s emulator-5556 shell dumpsys package network.mostro.app",
        &format!("versionName={version_bob}\nversionCode=1077\n"),
    );
    runner
}

#[tokio::test]
async fn brings_both_devices_up_and_tears_down_in_order() {
    let dir = tempfile::tempdir().unwrap();
    let runner = healthy_runner("1.3.2");
    let ui = FakeUi::default();

    let farm = DeviceFarm::bring_up(&runner, &ui, &sdk(dir.path()), &spec(dir.path()))
        .await
        .unwrap();

    assert_eq!(farm.devices().len(), 2);
    assert_eq!(farm.device(Actor::Bob).spec.system_port, 8201);
    assert_eq!(farm.device(Actor::Alice).version.version_name, "1.3.2");
    let sessions = ui.sessions.lock().unwrap().clone();
    assert_eq!(
        sessions.iter().map(|s| s.udid.as_str()).collect::<Vec<_>>(),
        ["emulator-5554", "emulator-5556"]
    );
    assert_eq!(runner.spawned().len(), 2, "two emulators");

    let items = farm.tear_down(&runner).await;
    assert!(items.iter().all(|i| i.ok), "{items:?}");
    assert_eq!(
        items
            .iter()
            .map(|i| i.resource.as_str())
            .collect::<Vec<_>>(),
        [
            "alice session",
            "bob session",
            "appium server",
            "alice emulator",
            "bob emulator"
        ]
    );
    assert_eq!(
        *ui.ended.lock().unwrap(),
        vec!["emulator-5554", "emulator-5556"]
    );
}

#[tokio::test]
async fn version_mismatch_shuts_emulators_down() {
    let dir = tempfile::tempdir().unwrap();
    let runner = healthy_runner("9.9.9");

    let err = DeviceFarm::bring_up(
        &runner,
        &FakeUi::default(),
        &sdk(dir.path()),
        &spec(dir.path()),
    )
    .await
    .unwrap_err();

    let FarmError::VersionMismatch { alice, bob } = &err else {
        panic!("{err:?}");
    };
    // Two builds can share a version name, so the code must be shown.
    for shown in [alice, bob] {
        assert!(shown.contains('('), "{shown} lacks the version code");
    }
    let kills: Vec<_> = runner
        .calls()
        .into_iter()
        .filter(|c| c.contains("emu kill"))
        .collect();
    assert_eq!(kills.len(), 2, "{kills:?}");
}

#[tokio::test]
async fn session_failure_stops_server_and_emulators() {
    let dir = tempfile::tempdir().unwrap();
    let runner = healthy_runner("1.3.2");
    let ui = FakeUi {
        fail_session_for: Some("emulator-5556".into()),
        ..FakeUi::default()
    };

    let err = DeviceFarm::bring_up(&runner, &ui, &sdk(dir.path()), &spec(dir.path()))
        .await
        .unwrap_err();

    assert!(matches!(err, FarmError::Ui(_)));
    let kills: Vec<_> = runner
        .calls()
        .into_iter()
        .filter(|c| c.contains("emu kill"))
        .collect();
    assert_eq!(kills.len(), 2);
}

#[tokio::test]
async fn one_boot_failure_shuts_the_other_down() {
    let dir = tempfile::tempdir().unwrap();
    let runner = healthy_runner("1.3.2");
    runner.script(
        "adb -s emulator-5556 shell getprop sys.boot_completed",
        "0\n",
    );

    let err = DeviceFarm::bring_up(
        &runner,
        &FakeUi::default(),
        &sdk(dir.path()),
        &spec(dir.path()),
    )
    .await
    .unwrap_err();

    assert!(matches!(
        err,
        FarmError::Emulator {
            actor: Actor::Bob,
            ..
        }
    ));
    let kills: Vec<_> = runner
        .calls()
        .into_iter()
        .filter(|c| c.contains("emu kill"))
        .collect();
    assert_eq!(kills, vec!["adb -s emulator-5554 emu kill"]);
}

#[tokio::test]
async fn missing_system_image_fails_before_any_boot() {
    let dir = tempfile::tempdir().unwrap();
    let runner = healthy_runner("1.3.2");

    let err = DeviceFarm::bring_up(
        &runner,
        &FakeUi::default(),
        &AndroidSdk::at(dir.path().join("nosdk")),
        &spec(dir.path()),
    )
    .await
    .unwrap_err();

    assert!(matches!(
        err,
        FarmError::Avd {
            actor: Actor::Alice,
            ..
        }
    ));
    assert!(runner.spawned().is_empty());
}
