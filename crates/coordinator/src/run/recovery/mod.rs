//! Durable desktop ownership and public UI recovery, separate from scenario narratives.
use super::*;
use crate::cli::recover_audit::{AuditedOrder, RelayAudit};
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
        if !options
            .stack
            .as_ref()
            .ok_or("recovery stack missing")?
            .matches_recorded(&journal.snapshot().stack)?
        {
            return Err(
                "recovery stack identity changed; refusing actions with retained identities".into(),
            );
        }
        return Ok(());
    }
    let home = (deps.env)("HOME").ok_or("HOME is required for protected desktop recovery")?;
    let base = FileJournal::directory(Path::new(&home))?;
    if let Some(isolation) = &options.recovery_isolation {
        let stack = options
            .stack
            .as_ref()
            .ok_or("isolation requires a current stack")?;
        isolation
            .revalidate(&base, config, stack, options.stack_dir.as_deref())
            .map_err(|error| error.to_string())?;
        // Persist new-run evidence before fresh identities start. Never edit
        // the old journal or infer its unknown artifact location.
        let path = Path::new(&config.run.artifacts_dir)
            .join(run.as_str())
            .join("recovery-isolation.json");
        let json = serde_json::to_string_pretty(isolation).map_err(|error| error.to_string())?;
        crate::artifacts::persist::atomic_json(&path, &json).map_err(|error| error.to_string())?;
    } else {
        FileJournal::refuse_pending(&base)?;
    }
    let snapshot = options
        .stack
        .as_ref()
        .ok_or("recovery requires a verified stack")?;
    let stack = snapshot.fingerprint()?;
    // The same audit the cleanup command repeats later: every desktop run
    // rules its own empty submissions out before asking the operator.
    desktop.relay_audit = Some(crate::cli::recover_audit::RelayAuditSource::for_run(
        snapshot,
        run,
        std::time::Duration::from_secs(config.mostro.health_timeout_secs),
    )?);
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
        // `absolute` fails only on an empty path, which configuration
        // validation already refuses.
        artifacts_dir: std::path::absolute(&config.run.artifacts_dir).ok(),
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

/// Started submissions that no later order discovery or operator statement
/// resolved, with the orders their actor already owned when they started.
pub(crate) fn open_submissions(journal: &FileJournal) -> Vec<(Actor, Vec<String>)> {
    let events = journal.snapshot().events;
    events
        .iter()
        .enumerate()
        .filter_map(|(index, event)| {
            let RecoveryEvent::SubmissionStarted {
                actor,
                known_orders,
            } = event
            else {
                return None;
            };
            let resolved = events[index + 1..].iter().any(|later| {
                matches!(later,
                    RecoveryEvent::OrderFound { actor: found, .. }
                    | RecoveryEvent::SubmissionAbandoned { actor: found } if found == actor)
            });
            (!resolved).then(|| (*actor, known_orders.clone()))
        })
        .collect()
}

/// Every order the journal already attributes to an actor.
pub(crate) fn found_orders(journal: &FileJournal) -> Vec<String> {
    journal
        .snapshot()
        .events
        .iter()
        .filter_map(|event| match event {
            RecoveryEvent::OrderFound { order, .. } => Some(order.id.clone()),
            _ => None,
        })
        .collect()
}

/// Every open submission with the orders the daemon published in the run's
/// window that neither the submission nor the journal already knows.
pub(crate) fn candidates<'a>(
    journal: &FileJournal,
    audit: &'a RelayAudit,
) -> Vec<(Actor, Vec<&'a AuditedOrder>)> {
    let found = found_orders(journal);
    open_submissions(journal)
        .into_iter()
        .map(|(actor, known_orders)| {
            let known: Vec<String> = known_orders
                .into_iter()
                .chain(found.iter().cloned())
                .collect();
            (actor, audit.unknown(&known))
        })
        .collect()
}

/// Actors whose open submission the relay rules out: the daemon's public
/// record shows no order the journal does not already know for the window.
pub(crate) fn relay_ruled_out(journal: &FileJournal, audit: &RelayAudit) -> Vec<Actor> {
    candidates(journal, audit)
        .into_iter()
        .filter_map(|(actor, unknown)| unknown.is_empty().then_some(actor))
        .collect()
}

/// Records as abandoned every submission still open after UI discovery that
/// a fresh relay `audit` rules out. Discovery ran first, so an order the app
/// lists always wins; the audit is taken after the retained clients ran, so
/// a request they finished late has had its chance to appear. Returns the
/// actors recorded.
pub(crate) fn abandon_ruled_out(
    journal: &FileJournal,
    audit: &RelayAudit,
) -> Result<Vec<Actor>, String> {
    let ruled_out = relay_ruled_out(journal, audit);
    for actor in &ruled_out {
        journal.record(RecoveryEvent::SubmissionAbandoned { actor: *actor })?;
    }
    Ok(ruled_out)
}

pub(crate) fn unresolved_submissions(journal: &FileJournal) -> bool {
    !open_submissions(journal).is_empty()
}

/// Settles the journal once the scenario and UI discovery are over. A failed
/// discovery is reported, never fatal on its own: the relay is the daemon's
/// record, so when the fresh `audit` shows no order for the window the
/// submission is abandoned whether or not the app could list its book, and
/// the failed discovery hid nothing. An order the app did list was recorded
/// by discovery first and always wins.
pub(crate) fn settle_after_scenario(
    journal: &FileJournal,
    discovery: Result<(), String>,
    audit: Option<Result<RelayAudit, String>>,
    checks: &mut mortsom_common::check::DoctorReport,
) {
    let audited = audit.map(|audit| {
        audit.and_then(|audit| abandon_ruled_out(journal, &audit).map(|actors| (audit, actors)))
    });
    let relay_shows_nothing = matches!(
        &audited,
        Some(Ok((audit, _))) if audit.unknown(&found_orders(journal)).is_empty()
    );
    if let Err(error) = discovery {
        if relay_shows_nothing {
            checks.push(
                "recovery-discovery",
                CheckStatus::Warn,
                format!("{error}; the relay shows no order for the run, so nothing was missed"),
            );
        } else {
            checks.push("recovery-discovery", CheckStatus::Fail, error);
        }
    }
    match audited {
        None => {}
        Some(Ok((_, actors))) => {
            for actor in actors {
                tracing::info!(actor = %actor, "submission produced no order: the relay shows none for the run");
            }
        }
        Some(Err(error)) => checks.push(
            "recovery-relay-audit",
            CheckStatus::Fail,
            format!("cannot rule the open submission out: {error}"),
        ),
    }
}

/// Why the protected profiles stay, and what resolves it, for the operator.
fn retained_reason(journal: &FileJournal, summary: &RunSummary) -> String {
    let open = open_submissions(journal);
    if !open.is_empty() {
        let run = journal.snapshot().run_id;
        let commands: Vec<String> = open
            .iter()
            .map(|(actor, _)| {
                let actor = actor.as_str();
                format!("`mortsom cleanup {run} --attribute-order {actor}=<order-id>` (or `{actor}=none`)")
            })
            .collect();
        let actors: Vec<&str> = open.iter().map(|(actor, _)| actor.as_str()).collect();
        return format!(
            "protected profiles retained: {} started an order submission the app never listed \
             and the relay could not rule out; rerun {}",
            actors.join(" and "),
            commands.join(" and ")
        );
    }
    if summary
        .cleanup
        .as_ref()
        .is_some_and(|cleanup| !cleanup.is_clean())
    {
        return "protected profiles retained: order cleanup did not finish; rerun `mortsom cleanup` \
                once the stack is healthy"
            .into();
    }
    "protected profiles retained; explicit recovery is required".into()
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
        let reason = retained_reason(journal, summary);
        summary
            .checks
            .push("recovery-storage", CheckStatus::Fail, reason);
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
    journal.close()?;
    crate::recovery_abandon::discard_prepared(&journal.snapshot())
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
