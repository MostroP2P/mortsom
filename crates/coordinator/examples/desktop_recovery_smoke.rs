//! Reopens two real desktop identities through fresh handles without trading.
use mortsom_adapter_mobile_v2::MobileV2Adapter;
use mortsom_android_driver::{avd::Actor, process::TokioRunner, ui::UiSession};
use mortsom_app_contract::{AppAdapter, DeviceContext, Identity, TestEnvironment};
use mortsom_common::{Redactor, RunId, check::DoctorReport};
use mortsom_coordinator::{
    Config,
    artifacts::RunArtifacts,
    bootstrap::bootstrap_session,
    desktop,
    doctor::RealHost,
    evidence::StepRecorder,
    fingerprint::CapturedBundle,
    lock::RunLock,
    recovery_store::{FileJournal, RecoveryManifest},
    run::{DesktopFactory, DesktopOptions, DesktopServer, RealDesktopFactory},
    stack::StackSnapshot,
};
use mortsom_desktop_driver::{Platform, storage::ensure_stopped};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

#[tokio::main]
async fn main() -> std::process::ExitCode {
    if run().await.is_err() {
        eprintln!("desktop recovery smoke failed; inspect retained recovery-probes evidence");
        return std::process::ExitCode::FAILURE;
    }
    std::process::ExitCode::SUCCESS
}

async fn run() -> Result<()> {
    let args: Vec<_> = std::env::args().collect();
    let platform: Platform = args.get(1).ok_or("provide linux or web")?.parse()?;
    let artifact = args
        .get(2)
        .ok_or("provide the rebuilt client artifact")?
        .into();
    let home = PathBuf::from(std::env::var("HOME")?).canonicalize()?;
    let _lock = RunLock::acquire_host(&home)?;
    let probe_base = home.join(".local/state/mortsom/recovery-probes");
    let (config, stack, mut options) = prepare(platform, artifact, &probe_base).await?;
    let journal = Arc::clone(options.recovery.as_ref().ok_or("missing journal")?);
    let artifacts = RunArtifacts::create(
        &probe_base.join("evidence"),
        journal.snapshot().run_id,
        Redactor::new().with_secrets(config.wallets.secrets()),
    )?;
    println!(
        "{platform}: protected probe {}; no orders or payments",
        artifacts.run_id()
    );
    let result = exercise(&config, &stack, &mut options, &artifacts).await;
    artifacts.write_json(
        "result.json",
        &serde_json::json!({
            "platform": platform, "passed": result.is_ok(), "orders": 0, "payments": 0,
            "error": result.as_ref().err().map(|error| error.to_string()),
        }),
    )?;
    result?;
    journal.confirm_resolution()?;
    journal.close()?;
    let root = journal.root().to_path_buf();
    drop(options);
    drop(journal);
    std::fs::remove_dir_all(&root)?;
    std::fs::File::open(&probe_base)?.sync_all()?;
    println!(
        "{platform}: both original identities restored through new handles; all clients/server stopped; protected profiles removed"
    );
    Ok(())
}

async fn prepare(
    platform: Platform,
    artifact: PathBuf,
    base: &Path,
) -> Result<(Config, StackSnapshot, DesktopOptions)> {
    let (config, stack) = desktop::load(
        Path::new("config/desktop.example.toml"),
        Path::new("stack"),
        &TokioRunner,
        &RealHost,
    )
    .await?;
    let options = desktop::prepare(
        &config,
        platform,
        Some(artifact),
        "Xvfb".into(),
        &TokioRunner,
        &RealHost,
    )
    .await?;
    let options = retain_bundle(options, &stack, base)?;
    Ok((config, stack, options))
}

fn retain_bundle(
    mut options: DesktopOptions,
    stack: &StackSnapshot,
    base: &Path,
) -> Result<DesktopOptions> {
    let executable = (options.platform == Platform::Linux)
        .then(|| {
            options
                .artifact
                .file_name()
                .and_then(|name| name.to_str())
                .map(str::to_owned)
        })
        .flatten();
    let journal = Arc::new(FileJournal::create(
        base,
        RecoveryManifest {
            run_id: RunId::new(),
            stack: stack.fingerprint()?,
            platform: options.platform,
            bundle: options.bundle.fingerprint().clone(),
            build: options.provenance.clone(),
            executable: executable.clone(),
            identities: BTreeMap::new(),
            origin: None,
            events: vec![],
            closed: false,
            resolution_verified: false,
        },
    )?);
    use std::os::unix::fs::DirBuilderExt;
    let actors = journal.root().join("actors");
    std::fs::DirBuilder::new().mode(0o700).create(&actors)?;
    std::fs::File::open(&actors)?.sync_all()?;
    std::fs::File::open(journal.root())?.sync_all()?;
    let bundle =
        CapturedBundle::capture_persistent(options.bundle.root(), &journal.root().join("bundle"))?;
    options.artifact = executable.map_or_else(
        || bundle.root().to_path_buf(),
        |name| bundle.root().join(name),
    );
    options.bundle = Arc::new(bundle);
    options.recovery = Some(journal);
    Ok(options)
}

async fn exercise(
    config: &Config,
    stack: &StackSnapshot,
    options: &mut DesktopOptions,
    artifacts: &RunArtifacts,
) -> Result<()> {
    let env = TestEnvironment {
        mostro_pubkey: config.mobile.mostro_pubkey.clone(),
        relay_url: format!("ws://127.0.0.1:{}", config.relay.host_port),
        lane: config.run.lane,
    };
    let adapter = MobileV2Adapter::new(&config.mobile.package, PathBuf::new(), &env.relay_url);
    let recorder = StepRecorder::default();
    round(config, options, &adapter, &env, artifacts, &recorder).await?;
    let root = options
        .recovery
        .as_ref()
        .ok_or("missing journal")?
        .root()
        .to_path_buf();
    let reopened = Arc::new(FileJournal::open(&root)?);
    let manifest = reopened.snapshot();
    if manifest.stack != stack.fingerprint()? {
        return Err("probe stack fingerprint changed".into());
    }
    let bundle = CapturedBundle::resume(&root.join("bundle"), manifest.bundle)?;
    options.artifact = manifest.executable.map_or_else(
        || bundle.root().to_path_buf(),
        |name| bundle.root().join(name),
    );
    options.bundle = Arc::new(bundle);
    options.recovery = Some(reopened);
    options.resume = true;
    println!(
        "{}: first handles dropped; reopening retained identities",
        options.platform
    );
    round(config, options, &adapter, &env, artifacts, &recorder).await
}

struct Clients {
    sessions: Vec<(Actor, Arc<dyn UiSession>)>,
    server: Option<Box<dyn DesktopServer>>,
}

impl Clients {
    async fn start(options: &DesktopOptions) -> Result<Self> {
        options.verify().await?;
        let mut clients = Self {
            sessions: vec![],
            server: None,
        };
        if options.platform == Platform::Web {
            clients.server = Some(RealDesktopFactory.server(options).await?);
        }
        for actor in Actor::ALL {
            match RealDesktopFactory.session(actor, options, clients.origin().as_deref()) {
                Ok(session) => clients.sessions.push((actor, session)),
                Err(error) => {
                    let cleanup = clients.stop().await;
                    return Err(format!("descriptor failed: {error}; cleanup: {cleanup:?}").into());
                }
            }
        }
        Ok(clients)
    }

    fn origin(&self) -> Option<String> {
        self.server.as_ref().map(|server| server.url().to_owned())
    }

    async fn stop(&mut self) -> Result<()> {
        let mut errors = Vec::new();
        for (actor, session) in &self.sessions {
            if session.end().await.is_err()
                && let Err(error) = session.end().await
            {
                errors.push(format!("{actor} shutdown failed: {error}"));
            }
        }
        if let Some(server) = self.server.as_mut()
            && let Err(error) = server.stop().await
        {
            errors.push(format!("server shutdown failed: {error}"));
        }
        if !errors.is_empty() {
            return Err(errors.join("; ").into());
        }
        self.sessions.clear();
        self.server = None;
        Ok(())
    }
}

async fn round(
    config: &Config,
    options: &DesktopOptions,
    adapter: &dyn AppAdapter,
    env: &TestEnvironment,
    artifacts: &RunArtifacts,
    recorder: &StepRecorder,
) -> Result<()> {
    let mut clients = Clients::start(options).await?;
    let result = inspect(config, options, adapter, env, artifacts, recorder, &clients).await;
    let cleanup = clients.stop().await;
    drop(clients);
    cleanup?;
    for actor in Actor::ALL {
        ensure_stopped(
            &options
                .recovery
                .as_ref()
                .ok_or("missing journal")?
                .root()
                .join("actors")
                .join(actor.as_str()),
        )?;
    }
    result
}

async fn inspect(
    config: &Config,
    options: &DesktopOptions,
    adapter: &dyn AppAdapter,
    env: &TestEnvironment,
    artifacts: &RunArtifacts,
    recorder: &StepRecorder,
    clients: &Clients,
) -> Result<()> {
    let journal = options.recovery.as_ref().ok_or("missing journal")?;
    if options.resume && clients.origin() != journal.snapshot().origin {
        return Err("resumed Web origin changed".into());
    }
    let mut identities = BTreeMap::new();
    for (actor, session) in &clients.sessions {
        let ctx = DeviceContext {
            actor: *actor,
            ui: session.as_ref(),
            ui_timeout: Duration::from_secs(config.ui_timeout_secs()),
        };
        let identity = if options.resume {
            read_retained(adapter, ctx, artifacts, recorder).await?
        } else {
            read_initial(config, adapter, ctx, env, artifacts, recorder).await?
        };
        identities.insert(actor.as_str().to_owned(), identity.public_key);
        println!(
            "{}: {actor} {} identity verified",
            options.platform,
            if options.resume {
                "retained"
            } else {
                "initial"
            }
        );
    }
    if options.resume {
        if identities != journal.snapshot().identities {
            return Err("resumed identities changed".into());
        }
    } else {
        journal.set_actors(identities, clients.origin())?;
    }
    Ok(())
}

async fn read_retained(
    adapter: &dyn AppAdapter,
    ctx: DeviceContext<'_>,
    artifacts: &RunArtifacts,
    recorder: &StepRecorder,
) -> Result<Identity> {
    ctx.ui.activate_app().await?;
    ctx.ui
        .wait_for("env.marker", Duration::from_secs(60))
        .await?;
    let identity = adapter.existing_identity(&ctx).await?;
    recorder
        .checkpoint(
            artifacts,
            ctx.actor.as_str(),
            "resumed-existing-identity",
            ctx.ui,
        )
        .await;
    Ok(identity)
}

async fn read_initial(
    config: &Config,
    adapter: &dyn AppAdapter,
    ctx: DeviceContext<'_>,
    env: &TestEnvironment,
    artifacts: &RunArtifacts,
    recorder: &StepRecorder,
) -> Result<Identity> {
    let actor = ctx.actor;
    let mut checks = DoctorReport::default();
    let setup =
        bootstrap_session(config, adapter, ctx, env, artifacts, recorder, &mut checks).await;
    artifacts.write_json(&format!("{}-bootstrap.json", actor.as_str()), &checks)?;
    if !checks.is_healthy()
        || setup.identity.is_none()
        || setup.identity != setup.identity_after_restart
    {
        return Err(format!("{actor} bootstrap or in-session persistence failed").into());
    }
    setup
        .identity
        .ok_or_else(|| "missing initial identity".into())
}
