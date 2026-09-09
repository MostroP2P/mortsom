//! Reopen protected desktop identities against the original verified stack.
use super::*;
use crate::recovery_store::FileJournal;

pub(super) async fn cleanup_desktop(
    config_path: &std::path::Path,
    checkout: &std::path::Path,
    run: &mortsom_common::RunId,
    attributions: &[String],
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
    Some(
        match recover(config_path, checkout, &root, attributions).await {
            Ok(code) => code,
            Err(error) => fail(error),
        },
    )
}
/// Records operator attributions `actor=order-id` as the order of that
/// actor's still-unresolved submission.
///
/// A submission whose order the app never listed (a Web client without a
/// trades store, a crash between publish and confirmation) leaves the journal
/// unresolved forever: discovery reads the app's trade list, and the relay
/// observer only follows orders it was told about. The operator, who can read
/// the daemon and relay directly, names the order here. The attribution is
/// only a name: cleanup still has to see that order closed on the relay, or
/// close it through the UI, before the journal can be resolved.
pub(super) fn attribute_orders(journal: &FileJournal, specs: &[String]) -> Result<(), String> {
    use mortsom_android_driver::avd::Actor;
    use mortsom_scenario_kit::journal::{RecoveryEvent, RecoveryJournal};
    for spec in specs {
        let (actor, order_id) = spec
            .split_once('=')
            .ok_or_else(|| format!("attribution `{spec}` must be `actor=order-id`"))?;
        let actor = Actor::ALL
            .into_iter()
            .find(|a| a.as_str() == actor)
            .ok_or_else(|| format!("attribution `{spec}`: unknown actor `{actor}`"))?;
        if !is_uuid_like(order_id) {
            return Err(format!(
                "attribution `{spec}`: `{order_id}` is not an order id"
            ));
        }
        let events = journal.events();
        let open = events.iter().enumerate().any(|(index, event)| {
            matches!(event, RecoveryEvent::SubmissionStarted { actor: a, .. } if *a == actor)
                && !events[index + 1..].iter().any(|later| {
                    matches!(later, RecoveryEvent::OrderFound { actor: a, .. } if *a == actor)
                })
        });
        if !open {
            return Err(format!(
                "attribution `{spec}`: {} has no unresolved submission in this journal",
                actor.as_str()
            ));
        }
        journal.record(RecoveryEvent::OrderFound {
            actor,
            order: mortsom_app_contract::OrderReference::new(order_id.to_owned()),
        })?;
        println!(
            "recovery: operator attributed order {order_id} to {}'s unresolved submission; \
             the relay or the UI must still show it closed",
            actor.as_str()
        );
    }
    Ok(())
}

fn is_uuid_like(value: &str) -> bool {
    value.len() == 36
        && value.chars().enumerate().all(|(i, c)| match i {
            8 | 13 | 18 | 23 => c == '-',
            _ => c.is_ascii_hexdigit(),
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
    attributions: &[String],
) -> Result<ExitCode, String> {
    let journal = Arc::new(FileJournal::open(root)?);
    if journal.snapshot().closed {
        println!("recovery {} already completed", journal.snapshot().run_id);
        return Ok(ExitCode::SUCCESS);
    }
    attribute_orders(&journal, attributions)?;
    let manifest = journal.snapshot();
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

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::recovery_store::RecoveryManifest;
    use mortsom_android_driver::avd::Actor;
    use mortsom_scenario_kit::journal::{RecoveryEvent, RecoveryJournal};

    fn journal_with(events: Vec<RecoveryEvent>) -> (tempfile::TempDir, FileJournal) {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("source");
        std::fs::create_dir(&source).unwrap();
        std::fs::write(source.join("index.html"), b"fixture").unwrap();
        let bundle = crate::fingerprint::CapturedBundle::capture(&source).unwrap();
        let stack: crate::stack::StackSnapshot = serde_json::from_value(serde_json::json!({
            "daemon_pubkey": "8171eb680049f62ce8747d203f57c3f71de6eea308af6f995363896a1e47cc51",
            "relay_url": "ws://127.0.0.1:7000", "wallet_endpoints": {},
            "daemon_image": {"image_id": format!("sha256:{}", "a".repeat(64)),
                "reference": "mostrop2p/mostro:v0.18.7", "repository_digest": null},
            "declared_limits": {"order_expiration_secs": 300, "hold_invoice_expiry_secs": 120,
                "buyer_invoice_deadline_secs": 120, "payout_invoice_min_expiry_secs": 300}
        }))
        .unwrap();
        let journal = FileJournal::create(
            &dir.path().join("recovery"),
            RecoveryManifest {
                run_id: mortsom_common::RunId::new(),
                stack: stack.fingerprint().unwrap(),
                platform: mortsom_desktop_driver::Platform::Web,
                bundle: bundle.fingerprint().clone(),
                build: crate::provenance::BuildProvenance::prebuilt(),
                executable: None,
                identities: Default::default(),
                origin: Some("http://127.0.0.1:1".into()),
                events,
                closed: false,
                resolution_verified: false,
            },
        )
        .unwrap();
        (dir, journal)
    }

    const ORDER: &str = "3d2c95a2-1209-4e9a-b7b1-10150154213d";

    #[test]
    fn an_attribution_names_the_order_of_an_unresolved_submission() {
        let (_dir, journal) = journal_with(vec![RecoveryEvent::SubmissionStarted {
            actor: Actor::Alice,
            known_orders: vec![],
        }]);
        attribute_orders(&journal, &[format!("alice={ORDER}")]).unwrap();
        assert!(journal.events().iter().any(|e| matches!(
            e,
            RecoveryEvent::OrderFound { actor: Actor::Alice, order } if order.id == ORDER
        )));
    }

    #[test]
    fn attributions_are_refused_without_an_open_submission_or_with_bad_syntax() {
        let (_dir, resolved) = journal_with(vec![
            RecoveryEvent::SubmissionStarted {
                actor: Actor::Alice,
                known_orders: vec![],
            },
            RecoveryEvent::OrderFound {
                actor: Actor::Alice,
                order: mortsom_app_contract::OrderReference::new("other"),
            },
        ]);
        assert!(attribute_orders(&resolved, &[format!("alice={ORDER}")]).is_err());

        let (_dir, open) = journal_with(vec![RecoveryEvent::SubmissionStarted {
            actor: Actor::Alice,
            known_orders: vec![],
        }]);
        for bad in [
            format!("bob={ORDER}"),
            format!("carol={ORDER}"),
            "alice=".to_owned(),
            ORDER.to_owned(),
            "alice=not a uuid".to_owned(),
        ] {
            assert!(attribute_orders(&open, std::slice::from_ref(&bad)).is_err(), "{bad}");
        }
        assert_eq!(
            open.events().len(),
            1,
            "a refused attribution records nothing"
        );
    }
}
