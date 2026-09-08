//! Scenario context ownership, cleanup verification and resource teardown.
use super::*;
impl World<'_> {
    /// Runs one scenario (with the retry policy) inside the live world.
    pub(super) async fn execute(
        &self,
        config: &Config,
        def: &ScenarioDef,
        summary: &mut RunSummary,
    ) -> ScenarioResult {
        let (Some(farm), Some(observer), Some(wallets)) =
            (&self.farm, &self.observer, &self.wallets)
        else {
            return ScenarioResult {
                name: def.name.into(),
                app_profile: summary.app_profile.clone(),
                app_version: summary.app_version.clone(),
                outcome: Outcome::Failed {
                    category: mortsom_common::FailureCategory::TestInfrastructure,
                    message: "world not ready".into(),
                },
                attempts: 0,
                duration: Duration::ZERO,
            };
        };
        let actors: Vec<ActorSession> = farm
            .sessions()
            .into_iter()
            .map(|(actor, ui)| ActorSession {
                actor,
                ui,
                wallet_label: actor.as_str().to_owned(),
            })
            .collect();
        let ctx = Context {
            journal: recovery::journal(self.desktop.as_ref()),
            adapter: Arc::clone(&self.deps.adapter),
            actors,
            env: self.env(),
            relay: Arc::clone(observer),
            wallets: Arc::clone(wallets),
            node_wallets: self.node_wallets.clone(),
            relay_control: self.relay_control(),
            budget: Arc::clone(&self.budget),
            cleanup: Arc::clone(&self.cleanup),
            timeline: Arc::clone(&self.timeline),
            evidence: Arc::new(ArtifactSink::new(
                self.artifacts.clone(),
                Arc::clone(&self.recorder),
            )),
            pending_holds: Mutex::new(BTreeMap::new()),
            mainnet: self.mainnet,
            amount_sats: config.run.amount_sats,
            fiat_code: config.run.fiat_code.clone(),
            fiat_amount: config.run.fiat_amount,
            payment_method: config.run.payment_method.clone(),
            ui_timeout: Duration::from_secs(config.ui_timeout_secs()),
            state_timeout: Duration::from_secs(config.run.state_timeout_secs),
            settle_timeout: Duration::from_secs(config.run.settle_timeout_secs),
            expiration_timeout: Duration::from_secs(config.run.expiration_timeout_secs),
            recovery: RecoveryTiming {
                quiet_window: Duration::from_secs(config.run.quiet_window_secs),
                background_secs: config.run.background_secs,
                outage: Duration::from_secs(config.run.outage_secs),
            },
        };
        if let Some(options) = self.desktop.as_ref().filter(|options| options.resume)
            && let Some(journal) = options.recovery.as_ref()
        {
            for event in journal.snapshot().events {
                if let mortsom_scenario_kit::journal::RecoveryEvent::OrderFound { actor, order } =
                    event
                {
                    ctx.cleanup.register(actor, &order);
                }
            }
        }
        let result = execute_scenario(
            def,
            &ctx,
            if self.desktop.is_some() {
                0
            } else {
                config.run.infra_retries
            },
            &summary.app_profile,
            summary.app_version.clone(),
        )
        .await;
        if let Some(journal) = self
            .desktop
            .as_ref()
            .and_then(|options| options.recovery.as_ref())
            && let Err(error) = recovery::discover(&ctx, journal).await
        {
            summary
                .checks
                .push("recovery-discovery", CheckStatus::Fail, error);
        }
        execute::sweep_cleanup(&ctx).await;
        if let Some(journal) = self
            .desktop
            .as_ref()
            .and_then(|options| options.recovery.as_ref())
            && let Err(error) = recovery::verify_payments(&ctx, journal).await
        {
            summary
                .checks
                .push("recovery-payments", CheckStatus::Fail, error);
        }
        result
    }

    /// Cleanup report, teardown of devices and relay, evidence flush.
    pub(super) async fn tear_down(&mut self, summary: &mut RunSummary) {
        summary.cleanup = Some(self.cleanup.report());
        summary.budget = Some(
            self.budget
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .totals(),
        );
        if let Some(observer) = self.observer.take() {
            for event in observer.snapshot() {
                self.timeline.push_at(
                    event.received_at_ms,
                    TimelineKind::RelayEvent,
                    None,
                    format!(
                        "relay: kind {} from {} (event {})",
                        event.kind,
                        short(&event.author),
                        short(&event.id)
                    ),
                    Some(event.id.clone()),
                );
                if let Err(e) = self.artifacts.append_jsonl(files::RELAY_EVENTS, &event) {
                    self.evidence_failed(summary, files::RELAY_EVENTS, e);
                }
            }
            observer.stop().await;
        }
        if let Some(wallets) = self.wallets.take() {
            let payments = serde_json::json!({
                "observations": wallets.snapshot(),
                "budget": summary.budget,
            });
            if let Err(e) = self.artifacts.write_json(files::WALLET_PAYMENTS, &payments) {
                self.evidence_failed(summary, files::WALLET_PAYMENTS, e);
            }
        }
        if let Some(farm) = self.farm.take() {
            summary.farm_teardown = farm.tear_down(self.deps.runner).await;
            let ok = summary.farm_teardown.iter().all(|i| i.ok);
            summary.checks.push(
                "device-teardown",
                if ok {
                    CheckStatus::Pass
                } else {
                    CheckStatus::Fail
                },
                format!("{} items", summary.farm_teardown.len()),
            );
        }
        if let Some(relay) = self.relay.take() {
            let (teardown, logs) = teardown_relay(self.deps.docker.as_ref(), &relay, false).await;
            if let Err(e) = self.artifacts.write_text("relay-container.log", &logs) {
                self.evidence_failed(summary, "relay-container.log", e);
            }
            summary.checks.push(
                "relay-teardown",
                if teardown.removed {
                    CheckStatus::Pass
                } else {
                    CheckStatus::Fail
                },
                teardown
                    .error
                    .clone()
                    .unwrap_or_else(|| "container removed".into()),
            );
            summary.relay_teardown = Some(teardown);
        }
        if let Err(e) = self.artifacts.write_text(
            files::TIMELINE,
            &self.timeline.render(&format!("Run {}", summary.run_id)),
        ) {
            self.evidence_failed(summary, files::TIMELINE, e);
        }
        if let Err(e) = self
            .artifacts
            .write_json(files::TIMELINE_JSON, &self.timeline.entries())
        {
            self.evidence_failed(summary, files::TIMELINE_JSON, e);
        }
        if let Some(journal) = self
            .desktop
            .as_ref()
            .and_then(|options| options.recovery.as_ref())
        {
            recovery::finish(journal, summary, self.artifacts);
        }
    }

    fn evidence_failed(&self, summary: &mut RunSummary, file: &str, error: std::io::Error) {
        let detail = self
            .artifacts
            .redact(&format!("cannot persist {file}: {error}"));
        error!(%detail, "teardown evidence failed");
        // Retention and acceptance inspect checks after every resource has been stopped.
        summary
            .checks
            .push("teardown-evidence", CheckStatus::Fail, detail);
    }
}
