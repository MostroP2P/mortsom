//! Preparation of v2 clients attached to an existing Ortsom stack.
use crate::{Config, run::DesktopOptions, stack::StackContext};
use mortsom_android_driver::process::{Command, CommandRunner, run_ok};
use mortsom_desktop_driver::Platform;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Loads explicit caps and lane, resolving the stack connection references.
pub async fn load(
    config: &Path,
    checkout: &Path,
    runner: &dyn CommandRunner,
    host: &dyn crate::doctor::HostProbe,
) -> Result<(Config, crate::stack::StackSnapshot), String> {
    let stack = StackContext::resolve(checkout, runner)
        .await
        .map_err(|e| e.to_string())?;
    let mut config =
        Config::load_with(config, |name| stack.lookup(name).or_else(|| host.env(name)))
            .map_err(|e| e.to_string())?;
    stack.configure_relay(&mut config);
    config.validate().map_err(|e| e.to_string())?;
    stack
        .snapshot()
        .validate_config(&config)
        .map_err(|e| e.to_string())?;
    if config.mobile.profile != "mobile-v2" {
        return Err("Linux and Web require mobile.profile = mobile-v2".into());
    }
    Ok((config, stack.snapshot()))
}

/// Resolves or builds the actual app artifact with the stack identity.
pub async fn prepare(
    config: &Config,
    platform: Platform,
    artifact: Option<PathBuf>,
    xvfb: PathBuf,
    runner: &dyn CommandRunner,
    host: &dyn crate::doctor::HostProbe,
) -> Result<DesktopOptions, String> {
    if platform == Platform::Android {
        return Err("Android uses the APK builder".into());
    }
    let artifact = match artifact {
        Some(path) => path.canonicalize().map_err(|e| e.to_string())?,
        None => build_artifact(config, platform, runner).await?,
    };
    validate_artifact(platform, &artifact, host)?;
    let bundle_root = if platform == Platform::Linux {
        artifact
            .parent()
            .ok_or("Linux artifact has no bundle directory")?
            .to_path_buf()
    } else {
        artifact.clone()
    };
    let bundle = tokio::task::spawn_blocking(move || {
        crate::fingerprint::CapturedBundle::capture(&bundle_root)
    })
    .await
    .map_err(|_| "bundle capture task failed".to_string())?
    .map_err(|error| format!("cannot capture app bundle: {error}"))?;
    let artifact = match platform {
        Platform::Linux => bundle
            .root()
            .join(artifact.file_name().ok_or("missing executable name")?),
        _ => bundle.root().to_path_buf(),
    };
    Ok(DesktopOptions {
        bundle: std::sync::Arc::new(bundle),
        platform,
        artifact,
        xvfb,
        chromedriver: resolved_tool(host, "MORTSOM_CHROMEDRIVER", "chromedriver"),
        browser: resolved_tool(host, "MORTSOM_BROWSER", "chromium"),
    })
}

async fn build_artifact(
    config: &Config,
    platform: Platform,
    runner: &dyn CommandRunner,
) -> Result<PathBuf, String> {
    let source = Path::new(&config.mobile.source_dir)
        .canonicalize()
        .map_err(|e| e.to_string())?;
    run_script(
        runner,
        &source,
        &["./scripts/frb-generate.sh"],
        Duration::from_secs(600),
    )
    .await?;
    if platform == Platform::Web {
        run_script(
            runner,
            &source,
            &["./scripts/build-web.sh", "--release"],
            Duration::from_secs(3600),
        )
        .await?;
    }
    let cmd = build_command(
        &source,
        platform,
        &config.mobile.mostro_pubkey,
        config.relay.host_port,
    );
    tracing::info!(%platform, "building v2 clients");
    run_ok(runner, &cmd, Duration::from_secs(3600))
        .await
        .map_err(build_error)?;
    Ok(source.join(match platform {
        Platform::Linux => "build/linux/x64/release/bundle/mostro",
        Platform::Web => "build/web",
        Platform::Android => return Err("Android uses the APK builder".into()),
    }))
}

async fn run_script(
    runner: &dyn CommandRunner,
    source: &Path,
    script: &[&str],
    timeout: Duration,
) -> Result<(), String> {
    let mut command = Command::new("env", &[&format!("--chdir={}", source.display())]);
    command.args.extend(script.iter().map(|s| (*s).to_string()));
    run_ok(runner, &command, timeout)
        .await
        .map_err(build_error)?;
    Ok(())
}

/// Validates the platform's required build outputs before any client starts.
pub fn validate_artifact(
    platform: Platform,
    artifact: &Path,
    host: &dyn crate::doctor::HostProbe,
) -> Result<(), String> {
    match platform {
        Platform::Linux if host.is_executable(artifact) => {
            let root = artifact
                .parent()
                .ok_or("Linux artifact has no bundle directory")?;
            for required in [
                "lib/libapp.so",
                "lib/libflutter_linux_gtk.so",
                "lib/librust.so",
                "lib/libflutter_secure_storage_linux_plugin.so",
                "lib/liburl_launcher_linux_plugin.so",
                "data/icudtl.dat",
                "data/flutter_assets/AssetManifest.bin",
            ] {
                if !host.is_file(&root.join(required)) {
                    return Err(format!("Linux bundle is missing required file {required}"));
                }
            }
            Ok(())
        }
        Platform::Web => {
            for required in [
                "index.html",
                "flutter_bootstrap.js",
                "main.dart.js",
                "pkg/rust.js",
                "pkg/rust_bg.wasm",
            ] {
                if !host.is_file(&artifact.join(required)) {
                    return Err(format!("Web artifact is missing required file {required}"));
                }
            }
            Ok(())
        }
        Platform::Linux => Err("Linux artifact must be an executable regular file".into()),
        Platform::Android => Err("Android uses the APK builder".into()),
    }
}

fn resolved_tool(host: &dyn crate::doctor::HostProbe, variable: &str, fallback: &str) -> PathBuf {
    let value = host.env(variable).unwrap_or_else(|| fallback.into());
    host.which(&value).unwrap_or_else(|| PathBuf::from(value))
}

fn build_error(error: mortsom_android_driver::process::ProcessError) -> String {
    mortsom_common::Redactor::default().redact(&error.to_string())
}

/// Non-secret Flutter build plan. No arbitrary defines can override isolation.
pub fn build_command(source: &Path, platform: Platform, pubkey: &str, relay_port: u16) -> Command {
    Command::new(
        "env",
        &[
            &format!("--chdir={}", source.display()),
            "flutter",
            "build",
            &platform.to_string(),
            "-t",
            "lib/main_mortsom.dart",
            "--dart-define=MORTSOM_TEST_ENV=true",
            &format!("--dart-define=MOSTRO_PUB_KEY={pubkey}"),
            &format!("--dart-define=MORTSOM_RELAYS=ws://127.0.0.1:{relay_port}"),
        ],
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn desktop_build_uses_the_actual_host_relay_and_test_entrypoint() {
        let cmd = build_command(Path::new("/app"), Platform::Linux, "public", 7001);
        assert!(cmd.args.contains(&"linux".into()));
        assert!(cmd.args.contains(&"lib/main_mortsom.dart".into()));
        assert!(
            cmd.args
                .contains(&"--dart-define=MORTSOM_RELAYS=ws://127.0.0.1:7001".into())
        );
        assert!(!cmd.display().contains("10.0.2.2"));
    }
}

#[cfg(test)]
#[path = "desktop_tests.rs"]
mod artifact_tests;
