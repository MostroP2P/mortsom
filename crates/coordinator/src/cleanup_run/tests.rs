#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::Mutex;

use async_trait::async_trait;
use mortsom_android_driver::process::tests_support::FakeRunner;
use mortsom_environment::docker::{ContainerSpec, DockerError};
use mortsom_scenario_kit::cleanup::CleanupReport;

use super::*;

const HEX: &str = "8171eb680049f62ce8747d203f57c3f71de6eea308af6f995363896a1e47cc51";
const ALICE: &str = "nostr+walletconnect://b889ff5b1513b641e2a139f661a661364979c5beee91842f8f0ef42ab558e9d4?relay=wss%3A%2F%2Frelay.damus.io&secret=71a8c14c1407c113601079c4302dab36460f0ccd0ad506f1f2dc73b5100e4f3c";
const BOB: &str = "nostr+walletconnect://b889ff5b1513b641e2a139f661a661364979c5beee91842f8f0ef42ab558e9d4?relay=wss%3A%2F%2Frelay.damus.io&secret=0000000000000000000000000000000000000000000000000000000000000001";

struct World {
    dir: tempfile::TempDir,
    config: Config,
}

fn world() -> World {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("sdk/platform-tools")).unwrap();
    let text = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../mortsom.toml.example"
    ))
    .unwrap()
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

fn env_for(world: &World, sdk: bool) -> impl Fn(&str) -> Option<String> + '_ {
    move |name: &str| match name {
        "ANDROID_HOME" if sdk => Some(world.dir.path().join("sdk").display().to_string()),
        _ => None,
    }
}

#[derive(Default)]
struct FakeDocker {
    containers: Mutex<Vec<String>>,
    removed: Mutex<Vec<String>>,
    refuse: bool,
}

#[async_trait]
impl ContainerRuntime for FakeDocker {
    async fn run(&self, _: &ContainerSpec) -> Result<String, DockerError> {
        unreachable!()
    }
    async fn is_running(&self, _: &str) -> Result<bool, DockerError> {
        unreachable!()
    }
    async fn stop_and_remove(&self, name: &str) -> Result<(), DockerError> {
        if self.refuse {
            return Err(DockerError::Command {
                command: "docker rm".into(),
                stderr: "permission denied".into(),
            });
        }
        self.containers.lock().unwrap().retain(|c| c != name);
        self.removed.lock().unwrap().push(name.to_owned());
        Ok(())
    }
    async fn restart(&self, _: &str) -> Result<(), DockerError> {
        unreachable!()
    }
    async fn logs(&self, _: &str) -> Result<String, DockerError> {
        unreachable!()
    }
    async fn list_names(&self, prefix: &str) -> Result<Vec<String>, DockerError> {
        Ok(self
            .containers
            .lock()
            .unwrap()
            .iter()
            .filter(|c| c.starts_with(prefix))
            .cloned()
            .collect())
    }
}

fn write_cleanup(world: &World, run_id: &str, unresolved: &[&str]) -> PathBuf {
    let run_dir = world.dir.path().join("artifacts").join(run_id);
    std::fs::create_dir_all(&run_dir).unwrap();
    let report = CleanupReport {
        entries: Vec::new(),
        unresolved: unresolved.iter().map(|s| (*s).to_owned()).collect(),
    };
    std::fs::write(
        run_dir.join(files::CLEANUP),
        serde_json::to_string(&report).unwrap(),
    )
    .unwrap();
    run_dir
}

#[tokio::test]
async fn removes_only_the_runs_container_and_mortsoms_own_emulators() {
    let world = world();
    let run_dir = write_cleanup(&world, "run-a", &[]);
    let docker = FakeDocker {
        containers: Mutex::new(vec![
            "mortsom-relay-run-a".into(),
            "mortsom-relay-run-b".into(),
        ]),
        ..Default::default()
    };
    let runner = FakeRunner::default();
    runner.script("adb -s emulator-5554 emu avd name", "mortsom-alice\nOK\n");
    runner.script("adb -s emulator-5554 emu kill", "OK: killing emulator\n");
    runner.script("adb -s emulator-5556 emu avd name", "Pixel_9_API_35\nOK\n");
    let env = env_for(&world, true);
    let deps = CleanupDeps {
        runner: &runner,
        docker: &docker,
        env: &env,
    };

    let summary = cleanup_run(&world.config, "run-a", &deps).await;

    assert!(summary.clean, "{}", summary.render());
    assert!(summary.run_dir_found && summary.cleanup_report_found);
    assert_eq!(
        *docker.removed.lock().unwrap(),
        vec!["mortsom-relay-run-a".to_owned()],
        "the other run's relay is untouched"
    );
    assert_eq!(summary.emulators[0].action, EmulatorAction::Killed);
    assert_eq!(summary.emulators[1].action, EmulatorAction::Foreign);
    assert!(
        !runner
            .calls()
            .iter()
            .any(|c| c.contains("emulator-5556 emu kill")),
        "a foreign emulator is never killed"
    );
    let written: CleanupRunSummary =
        serde_json::from_str(&std::fs::read_to_string(run_dir.join(CLEANUP_COMMAND_FILE)).unwrap())
            .unwrap();
    assert_eq!(written, summary);
}

#[tokio::test]
async fn reports_unresolved_orders_and_failures_instead_of_pretending() {
    let world = world();
    write_cleanup(&world, "run-c", &["order-9"]);
    let docker = FakeDocker {
        containers: Mutex::new(vec!["mortsom-relay-run-c".into()]),
        refuse: true,
        ..Default::default()
    };
    let runner = FakeRunner::default();
    runner.script("adb -s emulator-5554 emu avd name", "mortsom-alice\nOK\n");
    runner.script_failure("adb -s emulator-5554 emu kill", "KO: unknown command");
    let env = env_for(&world, true);
    let deps = CleanupDeps {
        runner: &runner,
        docker: &docker,
        env: &env,
    };

    let summary = cleanup_run(&world.config, "run-c", &deps).await;

    assert!(!summary.clean);
    assert_eq!(summary.unresolved_orders, vec!["order-9".to_owned()]);
    assert!(!summary.containers[0].removed);
    assert_eq!(summary.emulators[0].action, EmulatorAction::Failed);
    assert_eq!(summary.emulators[1].action, EmulatorAction::NotRunning);
    let text = summary.render();
    assert!(text.contains("UNRESOLVED order order-9"));
    assert!(text.contains("NOT removed"));
    assert!(text.contains("result: NOT clean"));
}

#[tokio::test]
async fn missing_run_dir_and_sdk_are_reported_not_hidden() {
    let world = world();
    let docker = FakeDocker::default();
    let runner = FakeRunner::default();
    let env = env_for(&world, false);
    let deps = CleanupDeps {
        runner: &runner,
        docker: &docker,
        env: &env,
    };

    let summary = cleanup_run(&world.config, "never-ran", &deps).await;

    assert!(!summary.run_dir_found);
    assert!(!summary.cleanup_report_found);
    assert!(summary.unresolved_orders.is_empty());
    assert!(summary.containers.is_empty());
    assert!(
        summary
            .emulators
            .iter()
            .all(|e| e.action == EmulatorAction::NoSdk)
    );
    assert!(!summary.clean, "an uninspected host is not reported clean");
    assert!(summary.render().contains("run directory: not found"));
}
