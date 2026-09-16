//! Preparation of v2 clients attached to the regtest stack.
use crate::{Config, run::DesktopOptions, stack::StackContext};
use mortsom_desktop_driver::Platform;
use mortsom_ui_driver::process::{Command, CommandRunner, run_ok};
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Loads explicit caps and lane, resolving the stack connection references.
pub async fn load(
    config: &Path,
    stack_dir: &Path,
    runner: &dyn CommandRunner,
    host: &dyn crate::doctor::HostProbe,
) -> Result<(Config, crate::stack::StackSnapshot), String> {
    load_with_source(config, stack_dir, runner, host, None).await
}

/// Loads an explicit source selection before configuration resolution and validation.
pub async fn load_with_source(
    config: &Path,
    stack_dir: &Path,
    runner: &dyn CommandRunner,
    host: &dyn crate::doctor::HostProbe,
    source: Option<&crate::config::AppSource>,
) -> Result<(Config, crate::stack::StackSnapshot), String> {
    let stack = StackContext::resolve(stack_dir, runner)
        .await
        .map_err(|e| e.to_string())?;
    let mut config = Config::load_with_source(
        config,
        |name| stack.lookup(name).or_else(|| host.env(name)),
        source,
    )
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
    let (config, managed) =
        resolve_source(config, platform, artifact.is_none(), &xvfb, runner, host).await?;
    let (artifact, mut provenance) = match artifact {
        Some(path) => (
            path.canonicalize().map_err(|e| e.to_string())?,
            crate::provenance::BuildProvenance::prebuilt(),
        ),
        None => {
            crate::doctor::require_build_prerequisites(&config, platform, &xvfb, host).await?;
            build_artifact(&config, platform, runner, host).await?
        }
    };
    if let Some(source) = &managed {
        if provenance
            .source_before
            .as_ref()
            .is_none_or(|identity| identity.revision != source.selection.revision || identity.dirty)
        {
            return Err("managed app source changed before the build".into());
        }
        provenance.remote_selection = Some(source.selection.clone());
    }
    validate_artifact(platform, &artifact, host)?;
    capture_options(platform, artifact, provenance, xvfb, host).await
}

async fn resolve_source(
    config: &Config,
    platform: Platform,
    needs_build: bool,
    xvfb: &Path,
    runner: &dyn CommandRunner,
    host: &dyn crate::doctor::HostProbe,
) -> Result<(Config, Option<crate::app_source::PreparedSource>), String> {
    let mut config = config.clone();
    let managed = if needs_build && config.mobile.source_dir.is_empty() {
        crate::doctor::desktop::require_tools(platform, xvfb, host)?;
        let selector =
            crate::app_source::Selector::parse(config.mobile.app_ref.as_deref().unwrap_or("main"))
                .map_err(|error| error.to_string())?;
        Some(
            crate::app_source::prepare(&selector, runner)
                .await
                .map_err(|error| error.to_string())?,
        )
    } else {
        None
    };
    if let Some(source) = &managed {
        config.mobile.source_dir = source.path.display().to_string();
        config.mobile.app_ref = None;
    }
    Ok((config, managed))
}

async fn capture_options(
    platform: Platform,
    artifact: PathBuf,
    provenance: crate::provenance::BuildProvenance,
    xvfb: PathBuf,
    host: &dyn crate::doctor::HostProbe,
) -> Result<DesktopOptions, String> {
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
    let web_tools = crate::web_tools::resolve(host);
    Ok(DesktopOptions {
        recovery: None,
        resume: false,
        bundle: std::sync::Arc::new(bundle),
        provenance,
        platform,
        artifact,
        xvfb,
        chromedriver: web_tools.chromedriver.path,
        browser: web_tools.browser.path,
        relay_ruled_out: Vec::new(),
    })
}

async fn build_artifact(
    config: &Config,
    platform: Platform,
    runner: &dyn CommandRunner,
    host: &dyn crate::doctor::HostProbe,
) -> Result<(PathBuf, crate::provenance::BuildProvenance), String> {
    let source = Path::new(&config.mobile.source_dir)
        .canonicalize()
        .map_err(|e| e.to_string())?;
    let mut provenance = crate::provenance::BuildProvenance::begin(&source, platform, host).await?;
    tracing::info!(%platform, "building v2 clients");
    for (command, timeout) in build_plan(
        &source,
        platform,
        &config.mobile.mostro_pubkey,
        config.relay.host_port,
        config.mobile.order_expiry_secs,
    ) {
        run_ok(runner, &command, timeout)
            .await
            .map_err(build_error)?;
        provenance.commands.push(
            std::iter::once(command.program.display().to_string())
                .chain(command.args.into_iter().map(|arg| {
                    if arg == format!("--chdir={}", source.display()) {
                        "--chdir=<source>".into()
                    } else {
                        arg
                    }
                }))
                .collect(),
        );
    }
    provenance.finish(&source, host).await?;
    let artifact = source.join(match platform {
        Platform::Linux => "build/linux/x64/release/bundle/mostro",
        Platform::Web => "build/web",
    });
    Ok((artifact, provenance))
}

fn build_plan(
    source: &Path,
    platform: Platform,
    pubkey: &str,
    relay_port: u16,
    order_expiry_secs: Option<u64>,
) -> Vec<(Command, Duration)> {
    let script = |args: &[&str]| {
        let mut command = Command::new("env", &[&format!("--chdir={}", source.display())]);
        command
            .args
            .extend(args.iter().map(|arg| (*arg).to_owned()));
        command
    };
    let mut commands = vec![
        (script(&["flutter", "pub", "get"]), Duration::from_secs(600)),
        (
            script(&["./scripts/frb-generate.sh"]),
            Duration::from_secs(600),
        ),
    ];
    if platform == Platform::Web {
        commands.push((
            script(&["./scripts/build-web.sh", "--release"]),
            Duration::from_secs(3600),
        ));
    }
    commands.push((
        build_command(source, platform, pubkey, relay_port, order_expiry_secs),
        Duration::from_secs(3600),
    ));
    commands
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
    }
}

fn build_error(error: mortsom_ui_driver::process::ProcessError) -> String {
    mortsom_common::Redactor::default().redact(&error.to_string())
}

/// Non-secret Flutter build plan. No arbitrary defines can override isolation.
///
/// `order_expiry_secs` is the untaken-order lifetime the stack declares
/// (`order_expiration_secs`): the build asks the daemon for it on every
/// order it creates, which is what makes that limit true, and what lets a
/// scenario about the daemon's pending-order clock finish inside a run.
pub fn build_command(
    source: &Path,
    platform: Platform,
    pubkey: &str,
    relay_port: u16,
    order_expiry_secs: Option<u64>,
) -> Command {
    let mut args = vec![
        format!("--chdir={}", source.display()),
        "flutter".to_owned(),
        "build".to_owned(),
        platform.to_string(),
        "-t".to_owned(),
        "lib/main_mortsom.dart".to_owned(),
        "--dart-define=MORTSOM_TEST_ENV=true".to_owned(),
        format!("--dart-define=MOSTRO_PUB_KEY={pubkey}"),
        format!("--dart-define=MORTSOM_RELAYS=ws://127.0.0.1:{relay_port}"),
    ];
    if let Some(secs) = order_expiry_secs {
        args.push(format!("--dart-define=MORTSOM_ORDER_EXPIRY_SECS={secs}"));
    }
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    Command::new("env", &args)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn desktop_build_uses_the_actual_host_relay_and_test_entrypoint() {
        let cmd = build_command(
            Path::new("/app"),
            Platform::Linux,
            "public",
            7001,
            Some(600),
        );
        assert!(
            cmd.args
                .contains(&"--dart-define=MORTSOM_ORDER_EXPIRY_SECS=600".into())
        );
        let plain = build_command(Path::new("/app"), Platform::Linux, "public", 7001, None);
        assert!(
            !plain
                .args
                .iter()
                .any(|arg| arg.contains("MORTSOM_ORDER_EXPIRY_SECS"))
        );
        assert!(cmd.args.contains(&"linux".into()));
        assert!(cmd.args.contains(&"lib/main_mortsom.dart".into()));
        assert!(
            cmd.args
                .contains(&"--dart-define=MORTSOM_RELAYS=ws://127.0.0.1:7001".into())
        );
    }
}

#[cfg(test)]
#[path = "desktop_tests.rs"]
mod artifact_tests;
