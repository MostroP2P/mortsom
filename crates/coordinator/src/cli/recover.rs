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
        let events = journal.events();
        let open_submission = events.iter().enumerate().find_map(|(index, event)| {
            let RecoveryEvent::SubmissionStarted {
                actor: a,
                known_orders,
            } = event
            else {
                return None;
            };
            let resolved = events[index + 1..].iter().any(|later| {
                matches!(later,
                    RecoveryEvent::OrderFound { actor: a, .. }
                    | RecoveryEvent::SubmissionAbandoned { actor: a } if *a == actor)
            });
            (*a == actor && !resolved).then(|| known_orders.clone())
        });
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
    stack_dir: &std::path::Path,
    root: &std::path::Path,
    attributions: &[String],
) -> Result<ExitCode, String> {
    let journal = Arc::new(FileJournal::open(root)?);
    if journal.snapshot().closed {
        println!("recovery {} already completed", journal.snapshot().run_id);
        return Ok(ExitCode::SUCCESS);
    }
    let manifest = journal.snapshot();
    if manifest.events.is_empty() || manifest.resolution_verified {
        for actor in mortsom_common::Actor::ALL {
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
        crate::desktop::load(config_path, stack_dir, &TokioRunner, &RealHost).await?;
    if stack.fingerprint()? != manifest.stack {
        return Err("recovery stack identity changed; preserve evidence and resolve the recorded environment first".into());
    }
    if !attributions.is_empty() {
        let since = manifest
            .run_id
            .started_at()
            .ok_or("cannot audit the relay: the run id carries no start time")?
            .timestamp()
            .max(0) as u64;
        let audit = super::recover_audit::fetch(
            &stack.relay_url,
            &stack.daemon_pubkey,
            since,
            std::time::Duration::from_secs(config.mostro.health_timeout_secs),
        )
        .await?;
        attribute_orders(&journal, attributions, &audit)?;
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
        recovery: Some(journal),
        resume: true,
        artifact,
        bundle,
        provenance: manifest.build,
        platform: manifest.platform,
        xvfb: tool("MORTSOM_XVFB", "Xvfb"),
        chromedriver: web_tools.chromedriver.path,
        browser: web_tools.browser.path,
    };
    let adapter = select_adapter();
    let env = |name: &str| std::env::var(name).ok();
    let net = RealNet::with_daemon_timeout(std::time::Duration::from_secs(
        config.mostro.health_timeout_secs,
    ));
    let deps = RunDeps {
        desktop_factory: &crate::run::RealDesktopFactory,
        net: Some(&net),
        docker: Arc::new(DockerCli::default()),
        probe: Arc::new(NostrProbe),
        adapter,
        env: &env,
        wallets: &super::run::wallet_factory,
        observer: &LiveObserver,
    };
    let options = RunOptions {
        desktop,
        stack: Some(stack),
        scenarios: Vec::new(),
        tag: None,
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
    use mortsom_common::Actor;
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
                "reference": format!("mostrop2p/mostro@sha256:{}", "b".repeat(64)),
                "repository_digest": format!("sha256:{}", "b".repeat(64))},
            "daemon_source":{"kind":"pinned-image"},
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
}
