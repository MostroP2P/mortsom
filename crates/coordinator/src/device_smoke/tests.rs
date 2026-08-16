#![allow(clippy::unwrap_used, clippy::expect_used)]

use async_trait::async_trait;
use mortsom_android_driver::appium::{AppiumError, SessionSpec};
use mortsom_android_driver::process::tests_support::FakeRunner;
use mortsom_android_driver::ui::{UiServer, UiSession};

use super::*;
use crate::artifacts::files;

fn fast() -> DeviceSmokeOptions {
    DeviceSmokeOptions {
        launch_timeout: Duration::from_secs(2),
        shutdown_grace: Duration::from_millis(10),
    }
}

const HEX: &str = "8171eb680049f62ce8747d203f57c3f71de6eea308af6f995363896a1e47cc51";
const ALICE: &str = "nostr+walletconnect://b889ff5b1513b641e2a139f661a661364979c5beee91842f8f0ef42ab558e9d4?relay=wss%3A%2F%2Frelay.damus.io&secret=71a8c14c1407c113601079c4302dab36460f0ccd0ad506f1f2dc73b5100e4f3c";
const BOB: &str = "nostr+walletconnect://b889ff5b1513b641e2a139f661a661364979c5beee91842f8f0ef42ab558e9d4?relay=wss%3A%2F%2Frelay.damus.io&secret=0000000000000000000000000000000000000000000000000000000000000001";

/// Temp workspace: config with a real APK file, an SDK root with the system
/// image directory, and an AVD home.
struct World {
    dir: tempfile::TempDir,
    config: Config,
}

fn world(apk_present: bool) -> World {
    let dir = tempfile::tempdir().unwrap();
    let apk = dir.path().join("app.apk");
    if apk_present {
        std::fs::write(&apk, b"apk").unwrap();
    }
    std::fs::create_dir_all(
        dir.path()
            .join("sdk/system-images/android-35/google_apis/x86_64"),
    )
    .unwrap();
    let text = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../mortsom.toml.example"
    ))
    .unwrap()
    .replace(
        "apk_path = \"\"",
        &format!("apk_path = \"{}\"", apk.display()),
    )
    .replace("boot_timeout_secs = 180", "boot_timeout_secs = 3")
    .replace(
        "artifacts_dir = \"./artifacts\"",
        &format!(
            "artifacts_dir = \"{}\"",
            dir.path().join("artifacts").display()
        ),
    );
    let config = Config::from_toml_str(&text, |name| match name {
        "MORTSOM_MOSTRO_PUBKEY" => Some(HEX.into()),
        "MORTSOM_MOSTRO_SETTINGS_FILE" => Some("/tmp/settings.toml".into()),
        "MORTSOM_ALICE_NWC" => Some(ALICE.into()),
        "MORTSOM_BOB_NWC" => Some(BOB.into()),
        _ => None,
    })
    .unwrap();
    World { dir, config }
}

fn env_for(world: &World) -> impl Fn(&str) -> Option<String> + '_ {
    move |name: &str| match name {
        "ANDROID_HOME" => Some(world.dir.path().join("sdk").display().to_string()),
        "ANDROID_AVD_HOME" => Some(world.dir.path().join("avd").display().to_string()),
        _ => None,
    }
}

fn runner() -> FakeRunner {
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
        runner.script(
            &format!("adb -s {serial} shell dumpsys package network.mostro.app"),
            "versionName=1.3.2\nversionCode=1077\n",
        );
        runner.script(
            &format!("adb -s {serial} shell dumpsys activity activities"),
            "  mResumedActivity: ActivityRecord{network.mostro.app/.MainActivity}\n",
        );
        runner.script(
            &format!("adb -s {serial} logcat -d"),
            &format!("I/app: started with {ALICE}\n"),
        );
    }
    runner
}

#[derive(Clone, Default)]
struct FakeUi {
    fail_capture: bool,
    xml: String,
}
struct FakeServer(FakeUi);
struct FakeSession(FakeUi);

#[async_trait]
impl UiDriver for FakeUi {
    async fn start_server(
        &self,
        _: &dyn CommandRunner,
        _: u16,
        _: &Path,
        _: Duration,
    ) -> Result<Box<dyn UiServer>, AppiumError> {
        Ok(Box::new(FakeServer(self.clone())))
    }
}
#[async_trait]
impl UiServer for FakeServer {
    async fn create_session(&self, _: &SessionSpec) -> Result<Box<dyn UiSession>, AppiumError> {
        Ok(Box::new(FakeSession(self.0.clone())))
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
        if self.0.fail_capture {
            Err(AppiumError::NotReady {
                url: "x".into(),
                timeout: Duration::ZERO,
            })
        } else {
            Ok(b"\x89PNG".to_vec())
        }
    }
    async fn page_source(&self) -> Result<String, AppiumError> {
        Ok(self.0.xml.clone())
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
        Ok(())
    }
}

#[tokio::test]
async fn smoke_records_both_launch_screens_and_tears_down() {
    let world = world(true);
    let runner = runner();
    let ui = FakeUi {
        fail_capture: false,
        xml: format!("<hierarchy>{ALICE}</hierarchy>"),
    };

    let summary = run_device_smoke(&world.config, &fast(), &runner, &ui, &env_for(&world))
        .await
        .unwrap();

    assert!(summary.passed, "{}", summary.checks.render());
    assert_eq!(summary.app_version.as_ref().unwrap().version_name, "1.3.2");
    let run_dir = world
        .dir
        .path()
        .join("artifacts")
        .join(summary.run_id.as_str());
    for actor in ["alice", "bob"] {
        assert!(
            run_dir
                .join(format!("{actor}/steps/01-launch-screen.png"))
                .is_file()
        );
        let xml = std::fs::read_to_string(run_dir.join(format!("{actor}/steps/01-launch-ui.xml")))
            .unwrap();
        assert!(!xml.contains("secret=71a8"), "hierarchy leaked a secret");
        let logcat =
            std::fs::read_to_string(run_dir.join(format!("device-logcat-{actor}.txt"))).unwrap();
        assert!(!logcat.contains("secret=71a8"));
    }
    assert!(run_dir.join(files::SUMMARY).is_file());
    assert!(run_dir.join(files::CLEANUP).is_file());
    assert_eq!(
        runner
            .calls()
            .iter()
            .filter(|c| c.contains("emu kill"))
            .count(),
        2
    );
}

#[tokio::test]
async fn missing_apk_fails_before_touching_devices() {
    let world = world(false);
    let runner = runner();

    let summary = run_device_smoke(
        &world.config,
        &fast(),
        &runner,
        &FakeUi::default(),
        &env_for(&world),
    )
    .await
    .unwrap();

    assert!(!summary.passed);
    assert!(
        summary
            .checks
            .checks
            .iter()
            .any(|c| c.name == "apk" && c.status == CheckStatus::Fail)
    );
    assert!(runner.spawned().is_empty());
}

#[tokio::test]
async fn capture_failure_is_a_failed_check_and_still_tears_down() {
    let world = world(true);
    let runner = runner();
    let ui = FakeUi {
        fail_capture: true,
        xml: "<hierarchy/>".into(),
    };

    let summary = run_device_smoke(&world.config, &fast(), &runner, &ui, &env_for(&world))
        .await
        .unwrap();

    assert!(!summary.passed);
    assert!(
        summary
            .checks
            .checks
            .iter()
            .any(|c| c.name == "alice launch" && c.status == CheckStatus::Fail)
    );
    assert!(summary.teardown.iter().all(|i| i.ok));
}

#[tokio::test]
async fn app_not_in_foreground_fails() {
    let world = world(true);
    let runner = runner();
    for serial in ["emulator-5554", "emulator-5556"] {
        runner.script(
            &format!("adb -s {serial} shell dumpsys activity activities"),
            "  mResumedActivity: ActivityRecord{com.android.launcher}\n",
        );
    }
    let ui = FakeUi {
        fail_capture: false,
        xml: "<hierarchy/>".into(),
    };

    let summary = tokio::time::timeout(
        Duration::from_secs(150),
        run_device_smoke(&world.config, &fast(), &runner, &ui, &env_for(&world)),
    )
    .await
    .unwrap()
    .unwrap();

    assert!(!summary.passed);
    let bob = summary
        .checks
        .checks
        .iter()
        .find(|c| c.name == "bob launch")
        .unwrap();
    assert!(bob.detail.contains("not in foreground"), "{bob:?}");
    // The waited-for timeout is the configured one, not the default.
    assert!(bob.detail.contains("2s"), "{bob:?}");
    assert!(!bob.detail.contains("120s"), "{bob:?}");
}

#[test]
fn foreground_detection_accepts_old_and_new_dumpsys_formats() {
    assert!(is_resumed(
        "  mResumedActivity: ActivityRecord{network.mostro.app/.MainActivity}",
        "network.mostro.app"
    ));
    assert!(is_resumed(
        "    topResumedActivity=ActivityRecord{network.mostro.app/.MainActivity t12}",
        "network.mostro.app"
    ));
    assert!(!is_resumed(
        "  mResumedActivity: ActivityRecord{com.android.launcher}",
        "network.mostro.app"
    ));
    assert!(!is_resumed(
        "  mFocusedApp=network.mostro.app",
        "network.mostro.app"
    ));
}
