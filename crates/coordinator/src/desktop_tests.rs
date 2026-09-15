#![allow(clippy::unwrap_used)]
use super::*;
use mortsom_ui_driver::process::tests_support::FakeRunner;

fn config(root: &Path) -> Config {
    Config::from_toml_str(
        include_str!("../../../config/desktop.example.toml"),
        |name| {
            Some(match name {
                "MORTSOM_MOSTRO_PUBKEY" => {
                    "8171eb680049f62ce8747d203f57c3f71de6eea308af6f995363896a1e47cc51".into()
                }
                "MORTSOM_ALICE_LND_HOST" => "https://127.0.0.1:10001".into(),
                "MORTSOM_BOB_LND_HOST" => "https://127.0.0.1:10002".into(),
                _ => {
                    let path = root.join(name);
                    std::fs::write(&path, b"fixture").unwrap();
                    path.display().to_string()
                }
            })
        },
    )
    .unwrap()
}

#[tokio::test]
async fn incomplete_or_wrong_platform_artifacts_are_rejected_before_launch() {
    let root = tempfile::tempdir().unwrap();
    let file = root.path().join("not-executable");
    std::fs::write(&file, b"not an executable").unwrap();
    for (platform, artifact) in [
        (Platform::Linux, root.path().to_path_buf()),
        (Platform::Linux, file.clone()),
        (Platform::Web, file),
        (Platform::Web, root.path().to_path_buf()),
    ] {
        let runner = FakeRunner::default();
        assert!(
            prepare(
                &config(root.path()),
                platform,
                Some(artifact),
                "Xvfb".into(),
                &runner,
                &crate::doctor::RealHost
            )
            .await
            .is_err()
        );
        assert!(runner.calls().is_empty());
    }
}

#[tokio::test]
async fn existing_artifact_does_not_require_source_or_invoke_a_build() {
    let root = tempfile::tempdir().unwrap();
    let artifact = root.path().join("mostro");
    std::fs::write(&artifact, b"prebuilt").unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&artifact, std::fs::Permissions::from_mode(0o700)).unwrap();
    for file in [
        "lib/libapp.so",
        "lib/libflutter_linux_gtk.so",
        "lib/librust.so",
        "lib/libflutter_secure_storage_linux_plugin.so",
        "lib/liburl_launcher_linux_plugin.so",
        "data/icudtl.dat",
        "data/flutter_assets/AssetManifest.bin",
    ] {
        let path = root.path().join(file);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, b"fixture").unwrap();
    }
    let mut config = config(root.path());
    config.mobile.source_dir = root.path().join("missing").display().to_string();
    let runner = FakeRunner::default();
    let options = prepare(
        &config,
        Platform::Linux,
        Some(artifact.clone()),
        "Xvfb".into(),
        &runner,
        &crate::doctor::RealHost,
    )
    .await
    .unwrap();
    assert_ne!(options.artifact, artifact);
    std::fs::write(&artifact, b"rebuilt after preparation").unwrap();
    assert_eq!(std::fs::read(&options.artifact).unwrap(), b"prebuilt");
    options.verify().await.unwrap();
    std::fs::set_permissions(&options.artifact, std::fs::Permissions::from_mode(0o400)).unwrap();
    assert!(
        options.verify().await.is_err(),
        "captured executable must remain launchable"
    );
    assert!(runner.calls().is_empty());
    for file in [
        "lib/libapp.so",
        "lib/libflutter_linux_gtk.so",
        "lib/librust.so",
        "lib/libflutter_secure_storage_linux_plugin.so",
        "lib/liburl_launcher_linux_plugin.so",
        "data/icudtl.dat",
        "data/flutter_assets/AssetManifest.bin",
    ] {
        let path = root.path().join(file);
        std::fs::remove_file(&path).unwrap();
        assert!(validate_artifact(Platform::Linux, &artifact, &crate::doctor::RealHost).is_err());
        std::fs::write(path, b"fixture").unwrap();
    }
}

#[tokio::test]
async fn binding_failure_stops_the_build_and_redacts_child_output() {
    let root = tempfile::tempdir().unwrap();
    let mut config = config(root.path());
    config.mobile.source_dir = root.path().display().to_string();
    let runner = FakeRunner::default();
    let credential = "nostr+walletconnect://wallet?relay=wss://relay&secret=private";
    runner.script_failure(
        &format!(
            "env --chdir={} ./scripts/frb-generate.sh",
            root.path().display()
        ),
        credential,
    );
    let error = prepare(
        &config,
        Platform::Web,
        None,
        "Xvfb".into(),
        &runner,
        &BuildHost { missing: None },
    )
    .await
    .unwrap_err();
    assert!(!error.contains(credential));
    assert_eq!(runner.calls().len(), 2);
    assert!(runner.calls()[1].ends_with("./scripts/frb-generate.sh"));
}

#[tokio::test]
async fn successful_build_command_without_outputs_is_rejected() {
    let root = tempfile::tempdir().unwrap();
    let mut config = config(root.path());
    config.mobile.source_dir = root.path().display().to_string();
    for platform in [Platform::Linux, Platform::Web] {
        let runner = FakeRunner::default();
        assert!(
            prepare(
                &config,
                platform,
                None,
                "Xvfb".into(),
                &runner,
                &BuildHost { missing: None }
            )
            .await
            .is_err()
        );
        let calls = runner.calls();
        assert!(calls[0].ends_with("flutter pub get"));
        assert!(calls[1].ends_with("./scripts/frb-generate.sh"));
        assert_eq!(calls.len(), if platform == Platform::Web { 4 } else { 3 });
        if platform == Platform::Web {
            assert!(calls[2].ends_with("./scripts/build-web.sh --release"));
        }
    }
}

#[tokio::test]
async fn missing_build_tool_stops_before_codegen_or_flutter_runs() {
    let root = tempfile::tempdir().unwrap();
    let mut config = config(root.path());
    config.mobile.source_dir = root.path().display().to_string();
    for (platform, missing) in [
        (Platform::Linux, "flutter"),
        (Platform::Linux, "ninja"),
        (Platform::Linux, "Xvfb"),
        (Platform::Web, "wasm-pack"),
        (Platform::Web, "chromedriver"),
    ] {
        let runner = FakeRunner::default();
        let host = BuildHost {
            missing: Some(missing),
        };
        let error = prepare(&config, platform, None, "Xvfb".into(), &runner, &host)
            .await
            .unwrap_err();
        assert!(
            runner.calls().is_empty(),
            "missing {missing} must prevent all builds"
        );
        assert!(error.contains(missing));
    }
}

#[tokio::test]
async fn a_custom_xvfb_path_satisfies_runtime_checks_before_the_build() {
    let root = tempfile::tempdir().unwrap();
    let mut config = config(root.path());
    config.mobile.source_dir = root.path().display().to_string();
    for executable in ["/custom/Xvfb", "./tools/Xvfb"] {
        let runner = FakeRunner::default();
        let host = BuildHost {
            missing: Some("Xvfb"),
        };
        let result = prepare(
            &config,
            Platform::Linux,
            None,
            executable.into(),
            &runner,
            &host,
        )
        .await;
        assert!(result.is_err(), "fake build has no outputs");
        assert_eq!(
            runner.calls().len(),
            3,
            "the selected {executable} permits generation/build"
        );
    }
}

struct BuildHost {
    missing: Option<&'static str>,
}

#[async_trait::async_trait]
impl crate::doctor::HostProbe for BuildHost {
    fn env(&self, _: &str) -> Option<String> {
        None
    }
    fn which(&self, name: &str) -> Option<PathBuf> {
        (self.missing != Some(name) && !name.contains('/')).then(|| PathBuf::from(name))
    }
    fn exists(&self, path: &Path) -> bool {
        path.exists()
    }
    fn is_file(&self, path: &Path) -> bool {
        [
            "pubspec.yaml",
            "lib/main_mortsom.dart",
            "scripts/frb-generate.sh",
            "scripts/build-web.sh",
            "lib/rustlib/src/rust/library/std/Cargo.toml",
        ]
        .iter()
        .any(|suffix| path.ends_with(suffix))
            || path.is_file()
    }
    fn is_executable(&self, _: &Path) -> bool {
        true
    }
    async fn run(
        &self,
        program: &Path,
        args: &[&str],
    ) -> Result<crate::doctor::CommandOutput, String> {
        let (program, args) = if program.ends_with("env") {
            (Path::new(args[1]), &args[2..])
        } else {
            (program, args)
        };
        let stdout = if program.ends_with("git") {
            if args.contains(&"rev-parse") {
                "a".repeat(40)
            } else {
                String::new()
            }
        } else if args.first() == Some(&"metadata") {
            serde_json::json!({"packages":[{"manifest_path":args.last().unwrap(), "version":"2.0.0"}]}).to_string()
        } else if program.ends_with("rustup") && args.contains(&"sysroot") {
            "/toolchain".into()
        } else {
            "tool 1.2.3".into()
        };
        Ok(crate::doctor::CommandOutput {
            success: true,
            stdout,
            stderr: String::new(),
        })
    }
}
