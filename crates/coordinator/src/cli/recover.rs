//! Reopen protected desktop identities against the original verified stack.
use super::*;
use crate::recovery_store::FileJournal;

pub(super) async fn cleanup_desktop(
    config_path: &std::path::Path,
    stack_dir: &std::path::Path,
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
        match recover(config_path, stack_dir, &root, attributions).await {
            Ok(code) => code,
            Err(error) => fail(error),
        },
    )
}
/// Records operator attributions for a still-unresolved submission, each
/// checked against what the daemon published on the relay during the run.
///
/// A submission whose order the app never listed (a Web client without a
/// trades store, a crash between publish and confirmation) leaves the journal
/// unresolved forever: discovery reads the app's trade list, and the relay
/// observer only follows orders it was told about. The operator names the
/// order here, or states with `none` that none was created. Neither is
/// taken on faith: `actor=order-id` must name an order the daemon published
/// during the run that the submission did not already know, and `actor=none`
/// is refused while the daemon shows any order created during the run that
/// the journal does not know and that is still open. Even then the
/// attribution only names the order: cleanup must still see it closed on
/// the relay, or close it through the UI, before the journal resolves.
pub(super) fn attribute_orders(
    journal: &FileJournal,
    specs: &[String],
    audit: &super::recover_audit::RelayAudit,
) -> Result<(), String> {
    use mortsom_common::Actor;
    use mortsom_scenario_kit::journal::{RecoveryEvent, RecoveryJournal};
    for spec in specs {
        let (actor, order_id) = spec.split_once('=').ok_or_else(|| {
            format!("attribution `{spec}` must be `actor=order-id` or `actor=none`")
        })?;
        let actor = Actor::ALL
            .into_iter()
            .find(|a| a.as_str() == actor)
            .ok_or_else(|| format!("attribution `{spec}`: unknown actor `{actor}`"))?;
        if order_id != "none" && !is_uuid_like(order_id) {
            return Err(format!(
                "attribution `{spec}`: `{order_id}` is not an order id (or `none`)"
            ));
        }
        let open_submission = crate::run::recovery::open_submissions(journal)
            .into_iter()
            .find_map(|(a, known_orders)| (a == actor).then_some(known_orders));
        let Some(known_orders) = open_submission else {
            return Err(format!(
                "attribution `{spec}`: {} has no unresolved submission in this journal",
                actor.as_str()
            ));
        };
        if order_id == "none" {
            let open = audit.open_unknown(&known_orders);
            if !open.is_empty() {
                let ids: Vec<String> = open
                    .iter()
                    .map(|o| format!("{} ({})", o.id, o.status))
                    .collect();
                return Err(format!(
                    "attribution `{spec}` refused: the daemon published orders during this run that \
                     the journal does not know and that are still open: {}; attribute the right one \
                     instead",
                    ids.join(", ")
                ));
            }
            journal.record(RecoveryEvent::SubmissionAbandoned { actor })?;
            println!(
                "recovery: {}'s unresolved submission produced no order (relay audited since {})",
                actor.as_str(),
                audit.since
            );
            continue;
        }
        if known_orders.iter().any(|known| known == order_id) {
            return Err(format!(
                "attribution `{spec}` refused: {order_id} existed before the submission started"
            ));
        }
        if !audit.published(order_id) {
            return Err(format!(
                "attribution `{spec}` refused: the daemon published no order {order_id} during this run"
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

/// One operator-facing paragraph per submission the relay could not rule
/// out, keyed by actor: the candidate orders and the exact commands that
/// resolve it.
pub(super) fn operator_hints(
    journal: &FileJournal,
    audit: &super::recover_audit::RelayAudit,
) -> Vec<(mortsom_common::Actor, String)> {
    let run = journal.snapshot().run_id;
    crate::run::recovery::candidates(journal, audit)
        .into_iter()
        .filter_map(|(actor, unknown)| {
            let first = unknown.first()?.id.clone();
            let candidates: Vec<String> = unknown
                .iter()
                .map(|order| format!("{} ({})", order.id, order.status))
                .collect();
            let name = actor.as_str();
            Some((
                actor,
                format!(
                    "recovery: {name}'s order submission is still unresolved. The daemon published \
                     these orders during the run that the journal does not know: {}.\n  If one is \
                     {name}'s: mortsom cleanup {run} --attribute-order {name}={first}\n  If none \
                     is:            mortsom cleanup {run} --attribute-order {name}=none",
                    candidates.join(", ")
                ),
            ))
        })
        .collect()
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
/// Proves an actor's retained storage inactive before it is removed. A CLI
/// actor's marker, left by an invocation a killed coordinator never saw end
/// (an interrupted `identity init`), is cleared first once no mostro-cli
/// process uses its store (spec §11); an app actor's marker always refuses.
fn ensure_inactive(
    platform: mortsom_desktop_driver::Platform,
    path: &std::path::Path,
) -> Result<(), String> {
    if platform == mortsom_desktop_driver::Platform::Cli {
        mortsom_cli_driver::runtime::reconcile(path).map_err(|e| e.to_string())?;
    }
    mortsom_desktop_driver::storage::ensure_stopped(path).map_err(|e| e.to_string())
}

/// [`ensure_inactive`] off the async runtime: a CLI actor's reconciliation
/// scans `/proc`, which blocks.
async fn ensure_inactive_off_runtime(
    platform: mortsom_desktop_driver::Platform,
    path: std::path::PathBuf,
) -> Result<(), String> {
    tokio::task::spawn_blocking(move || ensure_inactive(platform, &path))
        .await
        .map_err(|e| format!("the retained storage check failed: {e}"))?
}

async fn recover(
    config_path: &std::path::Path,
    stack_dir: &std::path::Path,
    root: &std::path::Path,
    attributions: &[String],
) -> Result<ExitCode, String> {
    let journal = Arc::new(FileJournal::open(root)?);
    let manifest = journal.snapshot();
    match manifest.state() {
        crate::recovery_store::RecoveryOutcome::Pending => {}
        crate::recovery_store::RecoveryOutcome::Resolved => {
            crate::recovery_abandon::discard_prepared(&manifest)?;
            println!("recovery {} already completed", manifest.run_id);
            return Ok(ExitCode::SUCCESS);
        }
        crate::recovery_store::RecoveryOutcome::AbandonedRegtest { .. } => {
            println!(
                "recovery {} was abandoned because its regtest stack was destroyed; {}",
                manifest.run_id,
                crate::recovery_abandon::NOT_VERIFIED
            );
            return Ok(ExitCode::from(1));
        }
    }
    if manifest.events.is_empty() || manifest.resolution_verified {
        for actor in mortsom_common::Actor::ALL {
            let path = root.join("actors").join(actor.as_str());
            if path.exists() {
                ensure_inactive_off_runtime(manifest.platform, path).await?;
            }
        }
        crate::run::recovery::remove_storage(&journal)?;
        println!(
            "recovery {}: verified inactive storage removed; no UI order actions required",
            manifest.run_id
        );
        return Ok(ExitCode::SUCCESS);
    }
    if let Some(hint) = super::abandon::destroyed_hint(&manifest, stack_dir).await {
        return Err(hint);
    }
    let (config, stack) =
        crate::desktop::load(config_path, stack_dir, &TokioRunner, &RealHost).await?;
    if !stack.matches_recorded(&manifest.stack)? {
        if manifest.environment.is_none() {
            eprintln!(
                "{}",
                crate::recovery_isolation::legacy_hint(&manifest.run_id)
            );
        }
        return Err("recovery stack identity changed; preserve evidence and resolve the recorded environment first".into());
    }
    let mut hints = Vec::new();
    let mut relay_audit = None;
    if !attributions.is_empty() || crate::run::recovery::unresolved_submissions(&journal) {
        let source = super::recover_audit::RelayAuditSource::for_run(
            &stack,
            &manifest.run_id,
            std::time::Duration::from_secs(config.mostro.health_timeout_secs),
        )?;
        let audit = source.fetch().await?;
        attribute_orders(&journal, attributions, &audit)?;
        for actor in crate::run::recovery::relay_ruled_out(&journal, &audit) {
            println!(
                "recovery: the relay shows no order the daemon published for {}'s submission \
                 since {}; it is closed once the app lists none and a fresh audit agrees",
                actor.as_str(),
                audit.since
            );
        }
        hints = operator_hints(&journal, &audit);
        relay_audit = Some(source);
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
    let web_tools = crate::web_tools::resolve(&RealHost);
    let desktop = crate::run::DesktopOptions {
        recovery: Some(Arc::clone(&journal)),
        resume: true,
        artifact,
        bundle,
        provenance: manifest.build,
        platform: manifest.platform,
        xvfb: tool("MORTSOM_XVFB", "Xvfb"),
        chromedriver: web_tools.chromedriver.path,
        browser: web_tools.browser.path,
        relay_audit,
    };
    let adapter = adapter_for(&desktop, &config).await?;
    let env = |name: &str| std::env::var(name).ok();
    let net = RealNet::with_daemon_timeout(std::time::Duration::from_secs(
        config.mostro.health_timeout_secs,
    ));
    let chain = super::run::owned_chain(stack_dir);
    let deps = RunDeps {
        desktop_factory: &crate::run::RealDesktopFactory,
        net: Some(&net),
        chain: Some(chain.as_ref()),
        docker: Arc::new(DockerCli::default()),
        probe: Arc::new(NostrProbe),
        adapter,
        env: &env,
        wallets: &super::run::wallet_factory,
        observer: &LiveObserver,
    };
    let options = RunOptions {
        recovery_isolation: None,
        desktop,
        stack: Some(stack),
        stack_dir: stack_dir.canonicalize().ok(),
        scenarios: Vec::new(),
        tag: None,
    };
    let summary = run_scenarios(&config, &options, &deps)
        .await
        .map_err(|error| error.to_string())?;
    print!("{}", summary.checks.render_with_color(use_color()));
    if !summary.passed {
        // Only what the run left open: discovery may have found a candidate.
        let still_open = crate::run::recovery::open_submissions(&journal);
        for (actor, hint) in &hints {
            if still_open.iter().any(|(open, _)| open == actor) {
                println!("{hint}");
            }
        }
    }
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
    use crate::run::recovery::relay_ruled_out;
    use mortsom_common::Actor;
    use mortsom_scenario_kit::journal::{RecoveryEvent, RecoveryJournal};

    fn retained_storage(
        platform: mortsom_desktop_driver::Platform,
    ) -> (tempfile::TempDir, std::path::PathBuf) {
        let parent = tempfile::tempdir().unwrap();
        let root = parent.path().canonicalize().unwrap().join("alice");
        mortsom_desktop_driver::storage::ActorStorage::Fresh(root.clone())
            .prepare(platform)
            .unwrap();
        (parent, root)
    }

    /// A CLI actor's storage `proved` inactive no longer carries its marker,
    /// unless this host's /proc proves no stop (mounted with hidepid, or the
    /// procfs of another PID namespace): then the storage stays refused.
    fn assert_proved_inactive(proved: Result<(), String>, root: &std::path::Path) {
        if let Err(error) = &proved
            && error.contains("/proc does not list every process")
        {
            assert!(mortsom_desktop_driver::storage::ensure_stopped(root).is_err());
            return;
        }
        proved.unwrap();
        mortsom_desktop_driver::storage::ensure_stopped(root).unwrap();
    }

    /// A coordinator killed during a CLI actor's `identity init` leaves a
    /// marker and a journal without events: once no mostro-cli process uses
    /// the store, cleanup proves it inactive instead of refusing forever.
    #[test]
    fn an_interrupted_cli_bootstrap_is_proved_inactive_once_its_process_is_gone() {
        let (_parent, root) = retained_storage(mortsom_desktop_driver::Platform::Cli);
        let _left_behind = mortsom_desktop_driver::storage::ActiveRuntime::begin(&root).unwrap();

        let proved = ensure_inactive(mortsom_desktop_driver::Platform::Cli, &root);

        assert_proved_inactive(proved, &root);
    }

    /// An app actor's marker may stand for a client that still runs: it is
    /// never cleared on this path.
    #[test]
    fn an_app_actor_marker_still_refuses() {
        let (_parent, root) = retained_storage(mortsom_desktop_driver::Platform::Linux);
        let _left_behind = mortsom_desktop_driver::storage::ActiveRuntime::begin(&root).unwrap();

        assert!(ensure_inactive(mortsom_desktop_driver::Platform::Linux, &root).is_err());
    }

    /// Cleanup runs the check on a blocking thread, since a CLI actor's
    /// reconciliation scans `/proc`, and gets its verdict back.
    #[tokio::test]
    async fn the_check_off_the_async_runtime_keeps_its_refusal() {
        let (_parent, root) = retained_storage(mortsom_desktop_driver::Platform::Linux);
        let _left_behind = mortsom_desktop_driver::storage::ActiveRuntime::begin(&root).unwrap();

        let checked =
            ensure_inactive_off_runtime(mortsom_desktop_driver::Platform::Linux, root.clone())
                .await;

        assert!(checked.is_err());
    }

    /// A CLI actor's reconciliation runs on that blocking thread too, and
    /// clears a marker no mostro-cli process backs.
    #[tokio::test]
    async fn the_check_off_the_async_runtime_reconciles_a_cli_marker() {
        let (_parent, root) = retained_storage(mortsom_desktop_driver::Platform::Cli);
        let _left_behind = mortsom_desktop_driver::storage::ActiveRuntime::begin(&root).unwrap();

        let checked =
            ensure_inactive_off_runtime(mortsom_desktop_driver::Platform::Cli, root.clone()).await;

        assert_proved_inactive(checked, &root);
    }

    fn stack_snapshot() -> crate::stack::StackSnapshot {
        serde_json::from_value(serde_json::json!({
            "daemon_pubkey": "8171eb680049f62ce8747d203f57c3f71de6eea308af6f995363896a1e47cc51",
            "relay_url": "ws://127.0.0.1:7500", "wallet_endpoints": {},
            "daemon_image": {"image_id": format!("sha256:{}", "a".repeat(64)),
                "reference": format!("mostrop2p/mostro@sha256:{}", "b".repeat(64)),
                "repository_digest": format!("sha256:{}", "b".repeat(64))},
            "daemon_source":{"kind":"pinned-image"},
            "declared_limits": {"order_expiration_secs": 300, "hold_invoice_expiry_secs": 120,
                "buyer_invoice_deadline_secs": 120, "payout_invoice_min_expiry_secs": 300}
        }))
        .unwrap()
    }

    fn journal_with(events: Vec<RecoveryEvent>) -> (tempfile::TempDir, FileJournal) {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("source");
        std::fs::create_dir(&source).unwrap();
        std::fs::write(source.join("index.html"), b"fixture").unwrap();
        let bundle = crate::fingerprint::CapturedBundle::capture(&source).unwrap();
        let stack = stack_snapshot();
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
                outcome: Some(crate::recovery_store::RecoveryOutcome::Pending),
                environment: None,
            },
        )
        .unwrap();
        (dir, journal)
    }

    const ORDER: &str = "3d2c95a2-1209-4e9a-b7b1-10150154213d";

    fn audit(orders: &[(&str, &str)]) -> super::super::recover_audit::RelayAudit {
        super::super::recover_audit::RelayAudit {
            since: 1,
            orders: orders
                .iter()
                .map(|(id, status)| super::super::recover_audit::AuditedOrder {
                    id: (*id).to_owned(),
                    status: (*status).to_owned(),
                    created_at: 2,
                })
                .collect(),
        }
    }

    #[test]
    fn an_attribution_names_the_order_of_an_unresolved_submission() {
        let (_dir, journal) = journal_with(vec![RecoveryEvent::SubmissionStarted {
            actor: Actor::Alice,
            known_orders: vec![],
        }]);
        attribute_orders(
            &journal,
            &[format!("alice={ORDER}")],
            &audit(&[(ORDER, "expired")]),
        )
        .unwrap();
        assert!(journal.events().iter().any(|e| matches!(
            e,
            RecoveryEvent::OrderFound { actor: Actor::Alice, order } if order.id == ORDER
        )));
    }

    #[test]
    fn none_records_that_a_submission_produced_no_order_and_closes_it() {
        let (_dir, journal) = journal_with(vec![RecoveryEvent::SubmissionStarted {
            actor: Actor::Alice,
            known_orders: vec![],
        }]);
        attribute_orders(&journal, &["alice=none".to_owned()], &audit(&[])).unwrap();
        assert!(journal.events().iter().any(|e| matches!(
            e,
            RecoveryEvent::SubmissionAbandoned {
                actor: Actor::Alice
            }
        )));
        assert!(!crate::run::recovery::unresolved_submissions(&journal));
        // Resolved once: a second statement has nothing open to attach to.
        assert!(attribute_orders(&journal, &["alice=none".to_owned()], &audit(&[])).is_err());
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
        assert!(
            attribute_orders(
                &resolved,
                &[format!("alice={ORDER}")],
                &audit(&[(ORDER, "pending")])
            )
            .is_err()
        );

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
            assert!(
                attribute_orders(
                    &open,
                    std::slice::from_ref(&bad),
                    &audit(&[(ORDER, "pending")])
                )
                .is_err(),
                "{bad}"
            );
        }
        assert_eq!(
            open.events().len(),
            1,
            "a refused attribution records nothing"
        );
    }

    /// `none` is a claim about the daemon, so the daemon is asked: while it
    /// shows an order created during the run that the journal does not know
    /// and that is still open, the claim is refused.
    #[test]
    fn none_is_refused_while_the_relay_shows_an_unknown_open_order() {
        let (_dir, journal) = journal_with(vec![RecoveryEvent::SubmissionStarted {
            actor: Actor::Alice,
            known_orders: vec!["known-before".into()],
        }]);
        let refused = attribute_orders(
            &journal,
            &["alice=none".to_owned()],
            &audit(&[("known-before", "pending"), (ORDER, "pending")]),
        )
        .unwrap_err();
        assert!(refused.contains(ORDER), "{refused}");
        assert_eq!(journal.events().len(), 1);

        // Closed orders and orders known before the submission do not count.
        attribute_orders(
            &journal,
            &["alice=none".to_owned()],
            &audit(&[("known-before", "pending"), (ORDER, "expired")]),
        )
        .unwrap();
    }

    /// An attributed id must be an order the daemon published during the
    /// run, and not one the submission already knew about.
    #[test]
    fn an_attributed_order_must_be_new_to_the_submission_and_on_the_relay() {
        let (_dir, journal) = journal_with(vec![RecoveryEvent::SubmissionStarted {
            actor: Actor::Alice,
            known_orders: vec![ORDER.into()],
        }]);
        let refused = attribute_orders(
            &journal,
            &[format!("alice={ORDER}")],
            &audit(&[(ORDER, "expired")]),
        )
        .unwrap_err();
        assert!(refused.contains("existed before"), "{refused}");

        let (_dir, fresh) = journal_with(vec![RecoveryEvent::SubmissionStarted {
            actor: Actor::Alice,
            known_orders: vec![],
        }]);
        let refused =
            attribute_orders(&fresh, &[format!("alice={ORDER}")], &audit(&[])).unwrap_err();
        assert!(refused.contains("published no order"), "{refused}");
        assert_eq!(fresh.events().len(), 1);
    }

    /// The relay is the daemon's public record. When it shows no order the
    /// journal does not already know for the run's window, the started
    /// submission is ruled out; the run records the abandonment once UI
    /// discovery lists nothing either.
    #[test]
    fn an_empty_relay_window_rules_a_started_submission_out_and_the_run_abandons_it() {
        let (_dir, journal) = journal_with(vec![RecoveryEvent::SubmissionStarted {
            actor: Actor::Alice,
            known_orders: vec!["known-before".into()],
        }]);
        let audit = audit(&[("known-before", "pending")]);
        assert_eq!(relay_ruled_out(&journal, &audit), vec![Actor::Alice]);
        assert_eq!(journal.events().len(), 1, "ruling out records nothing");
        assert_eq!(
            crate::run::recovery::abandon_ruled_out(&journal, &audit).unwrap(),
            vec![Actor::Alice]
        );
        assert!(journal.events().iter().any(|e| matches!(
            e,
            RecoveryEvent::SubmissionAbandoned {
                actor: Actor::Alice
            }
        )));
        assert!(!crate::run::recovery::unresolved_submissions(&journal));
        assert!(relay_ruled_out(&journal, &audit).is_empty());
        assert!(
            crate::run::recovery::abandon_ruled_out(&journal, &audit)
                .unwrap()
                .is_empty()
        );
    }

    /// An order the app lists after the relay audit wins: discovery resolved
    /// the submission, so nothing contradictory is recorded.
    #[test]
    fn an_order_discovered_by_the_app_is_never_contradicted_by_the_relay_audit() {
        let (_dir, journal) = journal_with(vec![RecoveryEvent::SubmissionStarted {
            actor: Actor::Alice,
            known_orders: vec![],
        }]);
        assert_eq!(relay_ruled_out(&journal, &audit(&[])), vec![Actor::Alice]);
        journal
            .record(RecoveryEvent::OrderFound {
                actor: Actor::Alice,
                order: mortsom_app_contract::OrderReference::new(ORDER),
            })
            .unwrap();
        assert!(
            crate::run::recovery::abandon_ruled_out(&journal, &audit(&[]))
                .unwrap()
                .is_empty()
        );
        assert!(
            !journal
                .events()
                .iter()
                .any(|e| matches!(e, RecoveryEvent::SubmissionAbandoned { .. }))
        );
    }

    /// A request the restarted client finished late shows up only in the
    /// audit taken after discovery; that fresh audit decides, not the first.
    #[test]
    fn an_order_that_appears_after_the_first_audit_blocks_abandonment() {
        let (_dir, journal) = journal_with(vec![RecoveryEvent::SubmissionStarted {
            actor: Actor::Alice,
            known_orders: vec![],
        }]);
        assert_eq!(relay_ruled_out(&journal, &audit(&[])), vec![Actor::Alice]);
        let fresh = audit(&[(ORDER, "pending")]);
        assert!(
            crate::run::recovery::abandon_ruled_out(&journal, &fresh)
                .unwrap()
                .is_empty()
        );
        assert!(crate::run::recovery::unresolved_submissions(&journal));
    }

    /// Any unknown order in the window, open or closed, may be the one the
    /// submission created: only the operator can attribute it.
    #[test]
    fn an_unknown_order_in_the_window_keeps_the_submission_unresolved() {
        for status in ["pending", "expired", "canceled"] {
            let (_dir, journal) = journal_with(vec![RecoveryEvent::SubmissionStarted {
                actor: Actor::Alice,
                known_orders: vec![],
            }]);
            assert!(
                relay_ruled_out(&journal, &audit(&[(ORDER, status)])).is_empty(),
                "{status}"
            );
            assert!(crate::run::recovery::unresolved_submissions(&journal));
        }
    }

    /// Orders the journal already found for either actor are not candidates
    /// for another actor's submission.
    #[test]
    fn orders_the_journal_already_found_do_not_block_ruling_out() {
        let (_dir, journal) = journal_with(vec![
            RecoveryEvent::OrderFound {
                actor: Actor::Bob,
                order: mortsom_app_contract::OrderReference::new(ORDER),
            },
            RecoveryEvent::SubmissionStarted {
                actor: Actor::Alice,
                known_orders: vec![],
            },
        ]);
        assert_eq!(
            relay_ruled_out(&journal, &audit(&[(ORDER, "pending")])),
            vec![Actor::Alice]
        );
    }

    #[test]
    fn the_operator_hint_names_the_actor_the_candidates_and_the_commands() {
        let (_dir, journal) = journal_with(vec![RecoveryEvent::SubmissionStarted {
            actor: Actor::Alice,
            known_orders: vec![],
        }]);
        let run = journal.snapshot().run_id;
        let hints = operator_hints(&journal, &audit(&[(ORDER, "expired")]));
        assert_eq!(hints.len(), 1);
        assert_eq!(hints[0].0, Actor::Alice);
        let hint = &hints[0].1;
        assert!(hint.contains("alice"), "{hint}");
        assert!(hint.contains(&format!("{ORDER} (expired)")), "{hint}");
        assert!(
            hint.contains(&format!(
                "mortsom cleanup {run} --attribute-order alice={ORDER}"
            )),
            "{hint}"
        );
        assert!(
            hint.contains(&format!(
                "mortsom cleanup {run} --attribute-order alice=none"
            )),
            "{hint}"
        );
        assert!(operator_hints(&journal, &audit(&[])).is_empty());
    }

    /// The run itself audits the relay after every desktop scenario, from
    /// the same stack facts and window the cleanup command uses later.
    #[test]
    fn a_run_builds_its_relay_audit_from_the_stack_and_its_own_start_time() {
        let (_dir, journal) = journal_with(vec![]);
        let stack = stack_snapshot();
        let run = "20260916T211043Z-0d814df0105c"
            .parse::<mortsom_common::RunId>()
            .unwrap();
        let source = super::super::recover_audit::RelayAuditSource::for_run(
            &stack,
            &run,
            std::time::Duration::from_secs(7),
        )
        .unwrap();
        assert_eq!(source.relay_url, stack.relay_url);
        assert_eq!(source.daemon_pubkey, stack.daemon_pubkey);
        assert_eq!(source.since, run.started_at().unwrap().timestamp() as u64);
        assert_eq!(source.timeout, std::time::Duration::from_secs(7));
        drop(journal);
        let legacy = "legacy-0d814df0105c"
            .parse::<mortsom_common::RunId>()
            .unwrap();
        let refused = super::super::recover_audit::RelayAuditSource::for_run(
            &stack,
            &legacy,
            std::time::Duration::from_secs(7),
        )
        .unwrap_err();
        assert!(refused.contains("start time"), "{refused}");
    }

    /// A scenario that dies on the order form leaves the app unable to reach
    /// its order book, so UI discovery fails. The relay is still the daemon's
    /// record: when it shows nothing for the window, the submission is
    /// abandoned anyway and the profiles are not retained for it.
    #[test]
    fn a_failed_ui_discovery_does_not_stop_the_relay_from_ruling_a_submission_out() {
        let (_dir, journal) = journal_with(vec![RecoveryEvent::SubmissionStarted {
            actor: Actor::Alice,
            known_orders: vec![],
        }]);
        let mut checks = mortsom_common::check::DoctorReport::default();
        crate::run::recovery::settle_after_scenario(
            &journal,
            Err("could not return to the order book".into()),
            Some(Ok(audit(&[]))),
            &mut checks,
        );
        assert!(!crate::run::recovery::unresolved_submissions(&journal));
        assert!(journal.events().iter().any(|e| matches!(
            e,
            RecoveryEvent::SubmissionAbandoned {
                actor: Actor::Alice
            }
        )));
        // The relay showed nothing the journal does not know, so the failed
        // discovery hid no order: it is reported, but keeps the run healthy.
        let names: Vec<(&str, mortsom_common::check::CheckStatus)> = checks
            .checks
            .iter()
            .map(|c| (c.name.as_str(), c.status))
            .collect();
        assert_eq!(
            names,
            [(
                "recovery-discovery",
                mortsom_common::check::CheckStatus::Warn
            )],
            "{checks:?}"
        );
        assert!(checks.is_healthy());
    }

    /// A failed discovery stays fatal while the relay shows an order the
    /// journal does not know: the app may own it and nobody looked.
    #[test]
    fn a_failed_ui_discovery_stays_fatal_when_the_relay_shows_an_unknown_order() {
        let (_dir, journal) = journal_with(vec![RecoveryEvent::SubmissionStarted {
            actor: Actor::Alice,
            known_orders: vec![],
        }]);
        let mut checks = mortsom_common::check::DoctorReport::default();
        crate::run::recovery::settle_after_scenario(
            &journal,
            Err("could not return to the order book".into()),
            Some(Ok(audit(&[(ORDER, "pending")]))),
            &mut checks,
        );
        assert!(crate::run::recovery::unresolved_submissions(&journal));
        assert!(!checks.is_healthy(), "{checks:?}");
        let mut checks = mortsom_common::check::DoctorReport::default();
        crate::run::recovery::settle_after_scenario(
            &journal,
            Err("could not return to the order book".into()),
            Some(Err("relay audit: fetch failed".into())),
            &mut checks,
        );
        assert_eq!(checks.count(mortsom_common::check::CheckStatus::Fail), 2);
    }

    /// An unknown order in the window still needs the operator, whatever
    /// discovery said; and a failed audit is reported, not treated as empty.
    #[test]
    fn settling_keeps_a_submission_open_when_the_relay_cannot_rule_it_out() {
        let (_dir, journal) = journal_with(vec![RecoveryEvent::SubmissionStarted {
            actor: Actor::Alice,
            known_orders: vec![],
        }]);
        let mut checks = mortsom_common::check::DoctorReport::default();
        crate::run::recovery::settle_after_scenario(
            &journal,
            Ok(()),
            Some(Ok(audit(&[(ORDER, "pending")]))),
            &mut checks,
        );
        assert!(crate::run::recovery::unresolved_submissions(&journal));
        assert!(checks.is_healthy(), "{checks:?}");

        crate::run::recovery::settle_after_scenario(
            &journal,
            Ok(()),
            Some(Err("relay audit: fetch failed".into())),
            &mut checks,
        );
        assert!(crate::run::recovery::unresolved_submissions(&journal));
        assert!(
            checks
                .checks
                .iter()
                .any(|c| c.name == "recovery-relay-audit" && c.detail.contains("fetch failed")),
            "{checks:?}"
        );

        // No audit source at all: nothing is inferred.
        let (_dir, journal) = journal_with(vec![RecoveryEvent::SubmissionStarted {
            actor: Actor::Alice,
            known_orders: vec![],
        }]);
        let mut checks = mortsom_common::check::DoctorReport::default();
        crate::run::recovery::settle_after_scenario(&journal, Ok(()), None, &mut checks);
        assert!(crate::run::recovery::unresolved_submissions(&journal));
        assert!(checks.is_healthy());
    }

    /// One empty snapshot is not proof: the daemon may publish the order a
    /// moment after a request the client sent before it died. Ruling out
    /// needs the relay empty across a bounded quiet interval, so the audit
    /// is taken twice, the second after the window has elapsed.
    #[tokio::test(start_paused = true)]
    async fn ruling_out_needs_the_relay_empty_across_the_quiet_window() {
        let (_dir, journal) = journal_with(vec![RecoveryEvent::SubmissionStarted {
            actor: Actor::Alice,
            known_orders: vec![],
        }]);
        let window = std::time::Duration::from_secs(10);
        let fetches = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let started = tokio::time::Instant::now();
        let audit_result = {
            let fetches = fetches.clone();
            crate::run::recovery::observe_relay_quiet(&journal, window, || {
                let fetches = fetches.clone();
                async move {
                    fetches.lock().unwrap().push(started.elapsed());
                    Ok(audit(&[]))
                }
            })
            .await
            .unwrap()
        };
        assert!(audit_result.orders.is_empty());
        assert_eq!(
            *fetches.lock().unwrap(),
            [std::time::Duration::ZERO, window]
        );

        // A late order shows up in the second audit and blocks the abandonment.
        let fetches = std::sync::Arc::new(std::sync::Mutex::new(0));
        let late = {
            let fetches = fetches.clone();
            crate::run::recovery::observe_relay_quiet(&journal, window, move || {
                let fetches = fetches.clone();
                async move {
                    let mut count = fetches.lock().unwrap();
                    *count += 1;
                    Ok(if *count == 1 {
                        audit(&[])
                    } else {
                        audit(&[(ORDER, "pending")])
                    })
                }
            })
            .await
            .unwrap()
        };
        assert_eq!(late.orders.len(), 1);
        assert!(relay_ruled_out(&journal, &late).is_empty());

        // A candidate in the first audit settles nothing, so no wait is spent.
        let fetches = std::sync::Arc::new(std::sync::Mutex::new(0));
        let before = tokio::time::Instant::now();
        let first = {
            let fetches = fetches.clone();
            crate::run::recovery::observe_relay_quiet(&journal, window, move || {
                let fetches = fetches.clone();
                async move {
                    *fetches.lock().unwrap() += 1;
                    Ok(audit(&[(ORDER, "pending")]))
                }
            })
            .await
            .unwrap()
        };
        assert_eq!(first.orders.len(), 1);
        assert_eq!(*fetches.lock().unwrap(), 1);
        assert_eq!(before.elapsed(), std::time::Duration::ZERO);
    }
}
