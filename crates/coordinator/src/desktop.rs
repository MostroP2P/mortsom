//! Preparation of v2 clients attached to the regtest stack.
use crate::build_cache::{BuildCache, Policy};
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
    load_with_source(config, stack_dir, runner, host, None, None).await
}

/// Loads with an explicit source selection and `--profile` in place of the
/// configured ones, replaced before configuration resolution and validation
/// ([`Config::load_with_source`]).
pub async fn load_with_source(
    config: &Path,
    stack_dir: &Path,
    runner: &dyn CommandRunner,
    host: &dyn crate::doctor::HostProbe,
    source: Option<&crate::config::AppSource>,
    profile: Option<&str>,
) -> Result<(Config, crate::stack::StackSnapshot), String> {
    let stack = StackContext::resolve(stack_dir, runner)
        .await
        .map_err(|e| e.to_string())?;
    let mut config = Config::load_with_source(
        config,
        |name| stack.lookup(name).or_else(|| host.env(name)),
        source,
        profile,
    )
    .map_err(|e| e.to_string())?;
    stack.configure_relay(&mut config);
    config.validate().map_err(|e| e.to_string())?;
    stack
        .snapshot()
        .validate_config(&config)
        .map_err(|e| e.to_string())?;
    Ok((config, stack.snapshot()))
}

/// Where a Linux source build leaves its bundle, relative to the checkout.
const LINUX_BUNDLE: &str = "build/linux/x64/release/bundle";
/// The Linux bundle's executable.
const LINUX_EXECUTABLE: &str = "mostro";
/// Where a Web source build leaves its bundle, relative to the checkout.
const WEB_BUNDLE: &str = "build/web";

/// Resolves, reuses or builds the actual app artifact with the stack identity.
pub async fn prepare(
    config: &Config,
    platform: Platform,
    artifact: Option<PathBuf>,
    xvfb: PathBuf,
    policy: Policy,
    runner: &dyn CommandRunner,
    host: &dyn crate::doctor::HostProbe,
) -> Result<DesktopOptions, String> {
    let (config, managed) =
        resolve_source(config, platform, artifact.is_none(), &xvfb, runner, host).await?;
    let cache = managed
        .as_ref()
        .and_then(|_| open_cache(Path::new(crate::build_cache::DIR)));
    let Obtained {
        artifact,
        mut provenance,
        expected,
        keep_as,
    } = match artifact {
        Some(path) => Obtained {
            artifact: path.canonicalize().map_err(|e| e.to_string())?,
            provenance: crate::provenance::BuildProvenance::prebuilt(),
            expected: None,
            keep_as: None,
        },
        None => {
            crate::doctor::require_build_prerequisites(&config, platform, &xvfb, host).await?;
            obtain_build(
                &config,
                platform,
                managed.is_some(),
                cache.as_ref(),
                policy,
                runner,
                host,
            )
            .await?
        }
    };
    record_selection(managed.as_ref(), &mut provenance)?;
    validate_artifact(platform, &artifact, host)?;
    let options = capture_options(platform, artifact, provenance, xvfb, host).await?;
    if expected.is_some_and(|kept| &kept != options.bundle.fingerprint()) {
        return Err("kept app bundle changed before it was captured".into());
    }
    if let (Some(cache), Some(key)) = (&cache, keep_as) {
        keep(cache, &key, &options);
    }
    Ok(options)
}

/// Refuses a build whose checkout was not the fetched clean head, then
/// records the selection that resolved it.
fn record_selection(
    managed: Option<&crate::app_source::PreparedSource>,
    provenance: &mut crate::provenance::BuildProvenance,
) -> Result<(), String> {
    let Some(source) = managed else {
        return Ok(());
    };
    if provenance
        .source_before
        .as_ref()
        .is_none_or(|identity| identity.revision != source.selection.revision || identity.dirty)
    {
        return Err("managed app source changed before the build".into());
    }
    provenance.remote_selection = Some(source.selection.clone());
    Ok(())
}

/// A built or kept artifact, with what to check or keep once captured.
struct Obtained {
    artifact: PathBuf,
    provenance: crate::provenance::BuildProvenance,
    /// The kept bundle's recorded identity, which the capture must match.
    expected: Option<crate::fingerprint::BundleFingerprint>,
    /// The key a fresh managed build is kept under once captured.
    keep_as: Option<String>,
}

/// Reuses the kept bundle of an identical managed build, or builds the source.
async fn obtain_build(
    config: &Config,
    platform: Platform,
    managed: bool,
    cache: Option<&BuildCache>,
    policy: Policy,
    runner: &dyn CommandRunner,
    host: &dyn crate::doctor::HostProbe,
) -> Result<Obtained, String> {
    let source = Path::new(&config.mobile.source_dir)
        .canonicalize()
        .map_err(|e| e.to_string())?;
    let provenance = crate::provenance::BuildProvenance::begin(&source, platform, host).await?;
    let plan = build_plan(
        &source,
        platform,
        bindings_of(&source, managed, runner).await,
        &config.mobile.mostro_pubkey,
        config.relay.host_port,
        config.mobile.order_expiry_secs,
    );
    let commands: Vec<Vec<String>> = plan
        .iter()
        .map(|(command, _)| recorded(command, &source))
        .collect();
    let key = cache.and(crate::build_cache::key(platform, &provenance, &commands));
    if let (Some(cache), Some(key), Policy::Reuse) = (cache, key.as_deref(), policy)
        && let Some(kept) = lookup(cache, key, platform, host)
    {
        tracing::info!(%platform, "reusing the kept app build");
        return Ok(Obtained {
            artifact: artifact_in(&kept.root, platform),
            provenance: kept.provenance,
            expected: Some(kept.fingerprint),
            keep_as: None,
        });
    }
    let (artifact, provenance) =
        build_artifact(&source, platform, provenance, plan, commands, runner, host).await?;
    Ok(Obtained {
        artifact,
        provenance,
        expected: None,
        keep_as: key,
    })
}

/// The kept bundles, or none when the directory cannot be used.
fn open_cache(root: &Path) -> Option<BuildCache> {
    BuildCache::open(root)
        .inspect_err(|error| tracing::warn!(%error, "kept app builds are unavailable"))
        .ok()
}

/// The kept build under `key` when it verifies and passes the artifact
/// checks, which its fingerprint does not cover (a Linux executable's mode).
/// One that fails them is discarded, so the source is built and kept again.
fn lookup(
    cache: &BuildCache,
    key: &str,
    platform: Platform,
    host: &dyn crate::doctor::HostProbe,
) -> Option<crate::build_cache::KeptBuild> {
    let kept = cache
        .lookup(key, std::time::SystemTime::now())
        .inspect_err(|error| tracing::warn!(%error, "cannot read the kept app build"))
        .ok()
        .flatten()?;
    let Err(error) = validate_artifact(platform, &artifact_in(&kept.root, platform), host) else {
        return Some(kept);
    };
    tracing::warn!(%error, "discarding a kept app build that fails the artifact checks");
    if let Err(error) = cache.discard(key) {
        tracing::warn!(%error, "cannot discard the kept app build");
    }
    None
}

/// Keeps a fresh managed build for a later identical invocation; the run
/// goes on with its own capture when the copy cannot be kept.
fn keep(cache: &BuildCache, key: &str, options: &DesktopOptions) {
    match cache.store(
        key,
        &options.bundle,
        &options.provenance,
        std::time::SystemTime::now(),
    ) {
        Ok(()) => tracing::info!("kept the app build for reuse"),
        Err(error) => tracing::warn!(%error, "cannot keep the app build for reuse"),
    }
}

/// The artifact a client launches from a bundle directory.
fn artifact_in(bundle: &Path, platform: Platform) -> PathBuf {
    match platform {
        Platform::Linux => bundle.join(LINUX_EXECUTABLE),
        Platform::Web => bundle.to_path_buf(),
        Platform::Cli => bundle.join(crate::mostro_cli::EXECUTABLE),
    }
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
        crate::doctor::desktop::require_tools(platform, xvfb, host).await?;
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
        relay_audit: None,
    })
}

async fn build_artifact(
    source: &Path,
    platform: Platform,
    mut provenance: crate::provenance::BuildProvenance,
    plan: Vec<(Command, Duration)>,
    commands: Vec<Vec<String>>,
    runner: &dyn CommandRunner,
    host: &dyn crate::doctor::HostProbe,
) -> Result<(PathBuf, crate::provenance::BuildProvenance), String> {
    tracing::info!(%platform, "building v2 clients");
    for (command, timeout) in plan {
        let started = std::time::Instant::now();
        let outcome = run_ok(runner, &command, timeout).await;
        let (step, elapsed) = (build_step(&command), started.elapsed());
        // Logged before a failure returns: a failed or timed-out step is
        // the one whose time matters most.
        match &outcome {
            Ok(_) => tracing::info!(%step, ?elapsed, "build step finished"),
            Err(_) => tracing::warn!(%step, ?elapsed, "build step failed"),
        }
        outcome.map_err(build_error)?;
    }
    provenance.commands = commands;
    provenance.finish(source, host).await?;
    let bundle = source.join(match platform {
        Platform::Linux => LINUX_BUNDLE,
        Platform::Web => WEB_BUNDLE,
        Platform::Cli => return Err("the cli profile is not built as a Flutter app".into()),
    });
    Ok((artifact_in(&bundle, platform), provenance))
}

/// What a build command runs, for the log: its words up to the first flag,
/// never the checkout path or the build defines.
fn build_step(command: &Command) -> String {
    command
        .args
        .iter()
        .skip_while(|arg| arg.starts_with("--chdir="))
        .take_while(|arg| !arg.starts_with('-'))
        .take(3)
        .map(String::as_str)
        .collect::<Vec<_>>()
        .join(" ")
}

/// A build command as provenance records it, with the checkout as `<source>`.
fn recorded(command: &Command, source: &Path) -> Vec<String> {
    let checkout = format!("--chdir={}", source.display());
    std::iter::once(command.program.display().to_string())
        .chain(command.args.iter().map(|arg| {
            if *arg == checkout {
                "--chdir=<source>".into()
            } else {
                arg.clone()
            }
        }))
        .collect()
}

/// Where a build's Flutter Rust Bridge bindings come from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Bindings {
    /// The checkout commits them; the build uses them as committed.
    Committed,
    /// `scripts/frb-generate.sh` writes them before the build.
    Generate,
}

/// A managed checkout that commits its bindings is built with them, as the
/// app's own release is. Any other is generated: a local checkout may carry
/// API edits its committed bindings do not.
async fn bindings_of(source: &Path, managed: bool, runner: &dyn CommandRunner) -> Bindings {
    if managed && crate::app_source::commits_bindings(source, runner).await {
        tracing::info!("building with the bindings the revision commits");
        Bindings::Committed
    } else {
        Bindings::Generate
    }
}

fn build_plan(
    source: &Path,
    platform: Platform,
    bindings: Bindings,
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
    let mut commands = vec![(script(&["flutter", "pub", "get"]), Duration::from_secs(600))];
    if bindings == Bindings::Generate {
        commands.push((
            script(&["./scripts/frb-generate.sh"]),
            Duration::from_secs(600),
        ));
    }
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
        Platform::Cli => crate::mostro_cli::validate_bundle(artifact),
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
    // The engine (CanvasKit) comes from the bundle, not gstatic.com: a
    // client must never depend on the internet to draw its first frame.
    if platform == Platform::Web {
        args.push("--no-web-resources-cdn".to_owned());
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

    #[test]
    fn a_build_step_is_logged_by_its_words_never_its_defines() {
        let linux = build_command(
            Path::new("/app"),
            Platform::Linux,
            "public",
            7001,
            Some(600),
        );
        assert_eq!(build_step(&linux), "flutter build linux");
        let plan = build_plan(
            Path::new("/app"),
            Platform::Linux,
            Bindings::Generate,
            "public",
            7001,
            None,
        );
        let steps: Vec<String> = plan
            .iter()
            .map(|(command, _)| build_step(command))
            .collect();
        assert_eq!(steps[..2], ["flutter pub get", "./scripts/frb-generate.sh"]);
        assert!(
            steps
                .iter()
                .all(|step| !step.contains("public") && !step.contains("/app"))
        );
    }

    #[test]
    fn committed_bindings_leave_codegen_out_of_the_plan() {
        let steps = |platform| {
            build_plan(
                Path::new("/app"),
                platform,
                Bindings::Committed,
                "public",
                7001,
                None,
            )
            .iter()
            .map(|(command, _)| build_step(command))
            .collect::<Vec<_>>()
        };

        assert_eq!(
            steps(Platform::Linux),
            ["flutter pub get", "flutter build linux"]
        );
        assert_eq!(
            steps(Platform::Web),
            [
                "flutter pub get",
                "./scripts/build-web.sh",
                "flutter build web"
            ]
        );
    }

    /// Flutter's Web build loads CanvasKit from gstatic.com unless told
    /// otherwise; a dropped fetch left Bob's page blank before the app ran
    /// (2026-09-25, `pending_order_expires` on Web). The bundle carries its
    /// own copy, served from the run's loopback origin.
    #[test]
    fn web_build_loads_its_engine_from_the_bundle_not_a_cdn() {
        let web = build_command(Path::new("/app"), Platform::Web, "public", 7001, None);
        assert!(web.args.contains(&"--no-web-resources-cdn".into()));
        let linux = build_command(Path::new("/app"), Platform::Linux, "public", 7001, None);
        assert!(!linux.args.iter().any(|arg| arg.contains("web-resources")));
    }
}

#[cfg(test)]
#[path = "desktop_tests.rs"]
mod artifact_tests;
