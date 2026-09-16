//! Durable desktop ownership and public UI recovery, separate from scenario narratives.
use super::*;
use crate::recovery_store::{FileJournal, RecoveryEnvironment, RecoveryManifest, WalletBackend};
use mortsom_common::Actor;
use mortsom_scenario_kit::journal::{RecoveryEvent, RecoveryJournal};

pub(super) fn prepare(
    config: &Config,
    options: &mut RunOptions,
    run: &RunId,
    deps: &RunDeps<'_>,
) -> Result<(), String> {
    let environment = environment(config, options);
    let desktop = &mut options.desktop;
    if desktop.resume {
        let journal = desktop
            .recovery
            .as_ref()
            .ok_or("recovery manifest missing")?;
        if options
            .stack
            .as_ref()
            .ok_or("recovery stack missing")?
            .fingerprint()?
            != journal.snapshot().stack
        {
            return Err(
                "recovery stack identity changed; refusing actions with retained identities".into(),
            );
        }
        return Ok(());
    }
    let home = (deps.env)("HOME").ok_or("HOME is required for protected desktop recovery")?;
    let base = FileJournal::directory(Path::new(&home))?;
    FileJournal::refuse_pending(&base)?;
    let stack = options
        .stack
        .as_ref()
        .ok_or("recovery requires a verified stack")?
        .fingerprint()?;
    let executable = (desktop.platform == mortsom_desktop_driver::Platform::Linux)
        .then(|| {
            desktop
                .artifact
                .file_name()
                .and_then(|s| s.to_str())
                .map(str::to_owned)
        })
        .flatten();
    let journal = Arc::new(FileJournal::create(
        &base,
        RecoveryManifest {
            run_id: run.clone(),
            stack,
            platform: desktop.platform,
            bundle: desktop.bundle.fingerprint().clone(),
            build: desktop.provenance.clone(),
            executable: executable.clone(),
            identities: BTreeMap::new(),
            origin: None,
            events: vec![],
            closed: false,
            resolution_verified: false,
            outcome: Some(crate::recovery_store::RecoveryOutcome::Pending),
            environment: Some(environment),
        },
    )?);
    crate::artifacts::persist::create_directory(&journal.root().join("actors"))
        .map_err(|e| e.to_string())?;
    let bundle = crate::fingerprint::CapturedBundle::capture_persistent(
        desktop.bundle.root(),
        &journal.root().join("bundle"),
    )
    .map_err(|e| e.to_string())?;
    desktop.artifact = executable.map_or_else(
        || bundle.root().to_path_buf(),
        |name| bundle.root().join(name),
    );
    desktop.bundle = Arc::new(bundle);
    desktop.recovery = Some(journal);
    Ok(())
}

/// Ownership, lane and wallet facts a later abandonment is checked against.
fn environment(config: &Config, options: &RunOptions) -> RecoveryEnvironment {
    let endpoints = options.stack.as_ref().map(|stack| &stack.wallet_endpoints);
    let stack_lnd = [
        ("alice", &config.wallets.alice),
        ("bob", &config.wallets.bob),
    ]
    .into_iter()
    .all(|(actor, wallet)| match wallet {
        WalletConfig::Lnd { host, .. } => {
            endpoints.and_then(|endpoints| endpoints.get(actor)) == Some(host)
        }
        _ => false,
    });
    RecoveryEnvironment {
        lane: config.run.lane,
        allow_mainnet: config.run.allow_mainnet,
        stack_dir: options.stack.as_ref().and(options.stack_dir.clone()),
        wallet_backend: if stack_lnd {
            WalletBackend::StackLnd
        } else {
            WalletBackend::External
        },
        node_networks: BTreeMap::new(),
    }
}

pub(super) fn journal(options: &DesktopOptions) -> Arc<dyn RecoveryJournal> {
    match options.recovery.as_ref() {
        Some(journal) => Arc::clone(journal) as Arc<dyn RecoveryJournal>,
        None => Arc::new(mortsom_scenario_kit::journal::NoJournal),
    }
}

pub(super) async fn discover(ctx: &Context, journal: &FileJournal) -> Result<(), String> {
    let history = journal.snapshot();
    for event in history
        .events
        .iter()
        .filter(|event| matches!(event, RecoveryEvent::SubmissionStarted { .. }))
    {
        let RecoveryEvent::SubmissionStarted {
            actor,
            known_orders,
        } = event
        else {
            continue;
        };
        let session = ctx.actor(*actor).map_err(|e| e.to_string())?;
        let ids = ctx
            .adapter
            .owned_orders(&session.device(ctx.ui_timeout))
            .await
            .map_err(|e| e.to_string())?;
        for id in ids.into_iter().filter(|id| !known_orders.contains(id)) {
            let order = mortsom_app_contract::OrderReference::new(id);
            ctx.cleanup.register(*actor, &order);
            if !history.events.iter().any(|event| matches!(event, RecoveryEvent::OrderFound { order: known, .. } if known.id == order.id)) {
                journal.record(RecoveryEvent::OrderFound { actor: *actor, order })?;
            }
        }
    }
    Ok(())
}

pub(crate) fn unresolved_submissions(journal: &FileJournal) -> bool {
    let events = journal.snapshot().events;
    events.iter().enumerate().any(|(index, event)| {
        if let RecoveryEvent::SubmissionStarted { actor, .. } = event {
            !events[index + 1..].iter().any(|event| {
                matches!(event,
                    RecoveryEvent::OrderFound { actor: found, .. }
                    | RecoveryEvent::SubmissionAbandoned { actor: found } if found == actor)
            })
        } else {
            false
        }
    })
}

pub(super) async fn resume_actor(
    world: &World<'_>,
    ctx: &mortsom_app_contract::DeviceContext<'_>,
    journal: &FileJournal,
) -> Result<ActorSetup, String> {
    let result = reopen_actor(world, ctx, journal).await;
    if result.is_err() {
        // The screen at the refusal is the only diagnostic a live recovery
        // leaves behind: the client closes right after this and the retained
        // profile is not reopened until the operator acts.
        world
            .recorder
            .checkpoint(
                world.artifacts,
                ctx.actor.as_str(),
                "recovery-identity-failed",
                ctx.ui,
            )
            .await;
    }
    result
}

async fn reopen_actor(
    world: &World<'_>,
    ctx: &mortsom_app_contract::DeviceContext<'_>,
    journal: &FileJournal,
) -> Result<ActorSetup, String> {
    ctx.ui.activate_app().await.map_err(|e| e.to_string())?;
    let identity = world
        .deps
        .adapter
        .existing_identity(ctx)
        .await
        .map_err(|e| e.to_string())?;
    let expected = journal
        .snapshot()
        .identities
        .get(ctx.actor.as_str())
        .cloned()
        .ok_or("original public identity was never recorded")?;
    if identity.public_key != expected {
        return Err("retained actor identity mismatch; refusing recovery actions".into());
    }
    world
        .deps
        .adapter
        .configure_node_and_relay(ctx, &world.env())
        .await
        .map_err(|e| e.to_string())?;
    Ok(ActorSetup {
        actor: ctx.actor.as_str().into(),
        identity: Some(identity.clone()),
        identity_after_restart: Some(identity),
        wallet: None,
        wallet_expected: false,
    })
}

mod payments;
pub(super) use payments::verify_payments;

pub(super) fn finish(journal: &FileJournal, summary: &mut RunSummary, artifacts: &RunArtifacts) {
    // Persist the recovery facts while profiles still exist. Failure joins the
    // other evidence checks and therefore prevents the deletion boundary.
    write_recovery_evidence(&journal.snapshot(), summary, artifacts);
    let can_remove = summary
        .cleanup
        .as_ref()
        .is_some_and(|cleanup| cleanup.is_clean())
        && !unresolved_submissions(journal)
        && summary.checks.is_healthy()
        && summary.farm_teardown.iter().all(|item| item.ok);
    let result = if can_remove {
        remove_storage(journal)
    } else {
        Ok(())
    };
    if let Err(error) = result {
        summary
            .checks
            .push("recovery-storage", CheckStatus::Fail, error);
    }
    let manifest = journal.snapshot();
    summary.recovery = Some(super::report::RecoveryStatus {
        original_run_id: manifest.run_id.clone(),
        retained: !manifest.closed,
        resolution_verified: manifest.resolution_verified,
        outcome: Some(manifest.state().as_str().to_owned()),
    });
    if can_remove {
        // Filesystem deletion and public reports cannot be one atomic change.
        // A failure here leaves the pre-deletion report conservatively retained;
        // the durable journal remains authoritative for completed resolution.
        write_recovery_evidence(&manifest, summary, artifacts);
    }
    if !manifest.closed {
        summary.checks.push(
            "recovery-storage",
            CheckStatus::Fail,
            "protected profiles retained; explicit recovery is required",
        );
    }
}

fn write_recovery_evidence(
    manifest: &RecoveryManifest,
    summary: &mut RunSummary,
    artifacts: &RunArtifacts,
) {
    let evidence = serde_json::json!({
        "run_id": manifest.run_id,
        "retained": !manifest.closed,
        "resolution_verified": manifest.resolution_verified,
        "outcome": manifest.state().as_str(),
        "events": manifest.events,
    });
    if let Err(error) = artifacts.write_json("recovery.json", &evidence) {
        summary.checks.push(
            "recovery-report",
            CheckStatus::Fail,
            artifacts.redact(&format!("cannot persist recovery.json: {error}")),
        );
    }
}

pub(crate) fn remove_storage(journal: &FileJournal) -> Result<(), String> {
    journal.confirm_resolution()?;
    for name in ["actors", "bundle"] {
        let path = journal.root().join(name);
        if path.exists() {
            std::fs::remove_dir_all(path).map_err(|e| e.to_string())?;
        }
    }
    std::fs::File::open(journal.root())
        .and_then(|file| file.sync_all())
        .map_err(|e| e.to_string())?;
    journal.close()
}

/// Internal cleanup narrative; never registered as an accepted scenario.
pub(super) static CLEANUP: ScenarioDef = ScenarioDef {
    name: "cleanup",
    tags: &[],
    requires: &[],
    timeout_secs: 60,
    moves_value: false,
    needs_app_wallet: false,
    needs_solver: false,
    run: |_| Box::pin(async { Ok(()) }),
};
