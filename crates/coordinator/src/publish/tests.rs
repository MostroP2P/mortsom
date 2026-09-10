#![allow(clippy::unwrap_used, clippy::expect_used)]

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

fn run_dir(world: &World, run_id: &str) -> PathBuf {
    let dir = world.dir.path().join("artifacts").join(run_id);
    std::fs::create_dir_all(dir.join("alice/steps")).unwrap();
    dir
}

const SUMMARY: &str = r#"{
  "run_id": "r1", "lane": "ci", "command": "run", "app_profile": "profile-b",
  "app_version": null, "mainnet_cleared": false, "relay": null, "actors": [],
  "results": [
    {"name": "happy_sell", "app_profile": "profile-b", "app_version": null,
     "outcome": {"outcome": "passed"}, "attempts": 2, "duration": {"secs": 61, "nanos": 0}},
    {"name": "cancel_pending", "app_profile": "profile-b", "app_version": null,
     "outcome": {"outcome": "failed", "category": "relay", "message": "relay silent, wallet nostr+walletconnect://b889ff5b1513b641e2a139f661a661364979c5beee91842f8f0ef42ab558e9d4?relay=wss%3A%2F%2Frelay.damus.io&secret=71a8c14c1407c113601079c4302dab36460f0ccd0ad506f1f2dc73b5100e4f3c"},
     "attempts": 1, "duration": {"secs": 5, "nanos": 0}}
  ],
  "checks": {"checks": []}, "cleanup": null, "budget": null, "farm_teardown": [],
  "relay_teardown": null, "passed": false
}"#;

#[test]
fn publishes_redacted_text_and_screenshots_by_policy_with_a_manifest() {
    let world = world();
    let dir = run_dir(&world, "r1");
    std::fs::write(dir.join(files::SUMMARY), SUMMARY).unwrap();
    std::fs::write(
        dir.join(files::TIMELINE),
        format!("step one\nleaked nsec1qyqszqgpqyqszqgpqyqszqgpqyqszqgpqyqszqgpqyqszqgpqyqs2mgd0\nsecret={}\n", "shh"),
    )
    .unwrap();
    std::fs::write(dir.join("alice/steps/01-screen.png"), b"\x89PNG").unwrap();
    std::fs::write(dir.join("alice/steps/01-ui.xml"), "<node text=\"ok\"/>").unwrap();
    std::fs::write(dir.join("relay.bin"), b"\x00\x01").unwrap();
    std::os::unix::fs::symlink("/etc/hostname", dir.join("escape.txt")).unwrap();
    let out = world.dir.path().join("bundle");

    let manifest = publish_run(&world.config, "r1", &out, PublishOptions::default()).unwrap();

    assert_eq!(manifest.lane, Some(Lane::Ci));
    assert_eq!(manifest.passed, Some(false));
    assert_eq!(manifest.scenarios.len(), 2);
    assert_eq!(manifest.scenarios[0].attempts, 2);
    assert_eq!(manifest.scenarios[1].category.as_deref(), Some("relay"));
    let detail = manifest.scenarios[1].detail.as_deref().unwrap();
    assert!(
        !detail.contains("71a8c14c1407c113"),
        "manifest detail is redacted"
    );
    assert!(
        !std::fs::read_to_string(out.join(MANIFEST_FILE))
            .unwrap()
            .contains("71a8c14c1407c113")
    );
    assert_eq!(
        manifest.redacted_here, 2,
        "timeline and summary had secret-shaped text"
    );
    assert!(manifest.skipped.contains(&"relay.bin".to_owned()));
    assert!(
        !out.join("escape.txt").exists(),
        "symlinks are not followed"
    );
    let timeline = std::fs::read_to_string(out.join(files::TIMELINE)).unwrap();
    assert!(!timeline.contains("nsec1"));
    assert!(!timeline.contains("secret=shh"));
    assert!(out.join("alice/steps/01-screen.png").is_file());
    let written: PublishManifest =
        serde_json::from_str(&std::fs::read_to_string(out.join(MANIFEST_FILE)).unwrap()).unwrap();
    assert_eq!(written, manifest);
    assert!(manifest.render().contains("happy_sell"));

    let none = world.dir.path().join("bundle-none");
    let manifest = publish_run(
        &world.config,
        "r1",
        &none,
        PublishOptions {
            screenshots: ScreenshotPolicy::None,
        },
    )
    .unwrap();
    assert!(!none.join("alice/steps/01-screen.png").exists());
    assert!(
        manifest
            .skipped
            .contains(&"alice/steps/01-screen.png".to_owned())
    );
    assert!(
        none.join("alice/steps/01-ui.xml").is_file(),
        "hierarchies are text"
    );
}

#[test]
fn a_configured_wallet_secret_in_evidence_is_scrubbed_never_published_raw() {
    let world = world();
    let dir = run_dir(&world, "r2");
    std::fs::write(dir.join("run.log"), format!("wallet {ALICE} connected")).unwrap();
    let out = world.dir.path().join("bundle");

    publish_run(&world.config, "r2", &out, PublishOptions::default()).unwrap();

    let log = std::fs::read_to_string(out.join("run.log")).unwrap();
    assert!(!log.contains("71a8c14c1407c113"));
    assert!(log.contains(mortsom_common::secret::REDACTED));
}

#[test]
fn missing_run_and_policy_parsing_are_explicit() {
    let world = world();
    let err = publish_run(
        &world.config,
        "nope",
        &world.dir.path().join("b"),
        PublishOptions::default(),
    )
    .unwrap_err();
    assert!(matches!(err, PublishError::MissingRun(_)));
    assert_eq!(
        "none".parse::<ScreenshotPolicy>().unwrap(),
        ScreenshotPolicy::None
    );
    assert!("some".parse::<ScreenshotPolicy>().is_err());
}

#[test]
fn a_leak_discards_the_partial_bundle_but_other_errors_keep_it() {
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("bundle");
    std::fs::create_dir_all(&out).unwrap();
    std::fs::write(out.join("run.log"), "half written").unwrap();

    let error = discard_bundle_on_leak(
        &out,
        PublishError::Leak {
            path: PathBuf::from("run.log"),
        },
    );

    assert!(matches!(error, PublishError::Leak { .. }));
    assert!(!out.exists(), "the leak gate promises a discarded bundle");

    std::fs::create_dir_all(&out).unwrap();
    let error = discard_bundle_on_leak(
        &out,
        PublishError::Io {
            path: out.clone(),
            source: std::io::Error::other("disk"),
        },
    );

    assert!(matches!(error, PublishError::Io { .. }));
    assert!(out.exists(), "other errors keep the bundle for inspection");
}
