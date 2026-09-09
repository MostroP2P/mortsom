//! Reopen protected desktop identities against the original verified stack.
use super::*;
use crate::recovery_store::FileJournal;

pub(super) async fn cleanup_desktop(
    config_path: &std::path::Path,
    checkout: &std::path::Path,
    run: &mortsom_common::RunId,
) -> Option<ExitCode> {
    let home = RealHost.env("HOME")?;
    let base = match FileJournal::directory(std::path::Path::new(&home)) {
        Ok(base) => base,
        Err(error) => return Some(fail(error)),
    };
    let root = base.join(run.as_str());
    if !root.exists() {
        return None;
    }
    Some(match recover(config_path, checkout, &root).await {
        Ok(code) => code,
        Err(error) => fail(error),
    })
}
fn fail(error: String) -> ExitCode {
    eprintln!("{error}");
    ExitCode::from(1)
}
async fn recover(
    config_path: &std::path::Path,
    checkout: &std::path::Path,
    root: &std::path::Path,
) -> Result<ExitCode, String> {
    let journal = Arc::new(FileJournal::open(root)?);
    let manifest = journal.snapshot();
    if manifest.closed {
        println!("recovery {} already completed", manifest.run_id);
        return Ok(ExitCode::SUCCESS);
    }
    if manifest.events.is_empty() || manifest.resolution_verified {
        for actor in mortsom_android_driver::avd::Actor::ALL {
            let path = root.join("actors").join(actor.as_str());
            if path.exists() {
                mortsom_desktop_driver::storage::ensure_stopped(&path)
                    .map_err(|e| e.to_string())?;
            }
        }
        crate::run::recovery::remove_storage(&journal)?;
        println!(
            "recovery {}: verified inactive storage removed; no UI order actions required",
            manifest.run_id
        );
        return Ok(ExitCode::SUCCESS);
    }
    let (config, stack) =
        crate::desktop::load(config_path, checkout, &TokioRunner, &RealHost).await?;
    if stack.fingerprint()? != manifest.stack {
        return Err("recovery stack identity changed; preserve evidence and resolve the recorded environment first".into());
    }
    let bundle = Arc::new(
        crate::fingerprint::CapturedBundle::resume(&root.join("bundle"), manifest.bundle)
            .map_err(|e| e.to_string())?,
    );
    let artifact = match manifest.executable {
        Some(name) => {
            if !matches!(
                std::path::Path::new(&name)
                    .components()
                    .collect::<Vec<_>>()
                    .as_slice(),
                [std::path::Component::Normal(_)]
            ) {
                return Err("invalid retained executable name".into());
            }
            bundle.root().join(name)
        }
        None => bundle.root().to_path_buf(),
    };
    let tool = |variable: &str, fallback: &str| {
        let command = RealHost.env(variable).unwrap_or_else(|| fallback.into());
        RealHost
            .which(&command)
            .unwrap_or_else(|| PathBuf::from(command))
    };
    let desktop = crate::run::DesktopOptions {
        recovery: Some(journal),
        resume: true,
        artifact,
        bundle,
        provenance: manifest.build,
        platform: manifest.platform,
        xvfb: tool("MORTSOM_XVFB", "Xvfb"),
        chromedriver: tool("MORTSOM_CHROMEDRIVER", "chromedriver"),
        browser: tool("MORTSOM_BROWSER", "chromium"),
    };
    let ui = AppiumDriver::new(PathBuf::new());
    let adapter = select_adapter(
        &config,
        PathBuf::new(),
        format!("ws://127.0.0.1:{}", config.relay.host_port),
    );
    let env = |name: &str| std::env::var(name).ok();
    let net = RealNet::with_daemon_timeout(std::time::Duration::from_secs(
        config.mostro.health_timeout_secs,
    ));
    let deps = RunDeps {
        desktop_factory: &crate::run::RealDesktopFactory,
        net: Some(&net),
        runner: &TokioRunner,
        ui: &ui,
        docker: Arc::new(DockerCli::default()),
        probe: Arc::new(NostrProbe),
        adapter,
        env: &env,
        wallets: &super::run::wallet_factory,
        observer: &LiveObserver,
    };
    let options = RunOptions {
        desktop: Some(desktop),
        stack: Some(stack),
        ..Default::default()
    };
    let summary = run_scenarios(&config, &options, &deps).await?;
    print!("{}", summary.checks.render_with_color(use_color()));
    println!(
        "cleanup evidence: {}/{}",
        config.run.artifacts_dir, summary.run_id
    );
    Ok(if summary.passed {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(1)
    })
}
