//! Run command wiring and actor wallet connections.
use super::*;

pub(super) async fn run_command(
    config_path: &std::path::Path,
    stack_dir: &std::path::Path,
    selection: RunSelection,
) -> ExitCode {
    let RunSelection {
        preserve_legacy_recovery,
        scenarios,
        tag,
        platform,
        artifact,
        source,
        xvfb,
    } = selection;
    let mut prepared = super::prepared::InvocationBuild::default();
    mortsom_scenarios::link();
    let selected = match mortsom_scenario_kit::registry::select(&scenarios, tag.as_deref()) {
        Ok(selected) => selected,
        Err(error) => {
            eprintln!("{error}");
            return ExitCode::from(2);
        }
    };
    if selected.len() == 1 {
        return run_once(
            config_path,
            stack_dir,
            RunSelection {
                preserve_legacy_recovery,
                scenarios,
                tag,
                platform,
                artifact,
                source,
                xvfb,
            },
            &mut prepared,
        )
        .await
        .code;
    }
    // A run starts fresh actors and resolves everything it created before
    // it ends, so a family is a sequence of such runs, one scenario each,
    // and never one run sharing Alice and Bob across scenarios.
    let mut batch = Batch::default();
    for def in &selected {
        let attempt = run_once(
            config_path,
            stack_dir,
            RunSelection {
                preserve_legacy_recovery: preserve_legacy_recovery.clone(),
                scenarios: vec![def.name.to_owned()],
                tag: None,
                platform,
                artifact: artifact.clone(),
                source: source.clone(),
                xvfb: xvfb.clone(),
            },
            &mut prepared,
        )
        .await;
        let started = attempt.summary.is_some();
        batch.record(def.name, attempt.view(def.name));
        if !started {
            // The run never started: the environment refused it, and the
            // refusal (a retained journal, a missing stack) holds for the
            // rest of the batch as well.
            batch.not_run(selected.iter().skip(batch.finished()).map(|d| d.name));
            break;
        }
    }
    println!("{}", batch.render());
    batch.code()
}

/// What one `run` invocation was asked to do.
pub(super) struct RunSelection {
    pub preserve_legacy_recovery: Vec<mortsom_common::RunId>,
    pub scenarios: Vec<String>,
    pub tag: Option<String>,
    pub platform: mortsom_desktop_driver::Platform,
    pub artifact: Option<PathBuf>,
    pub xvfb: PathBuf,
    pub source: SourceOverrides,
}

/// How one run ended: its exit code, and its summary when it ran at all.
struct Attempt {
    code: ExitCode,
    summary: Option<crate::run::RunSummary>,
}

impl Attempt {
    fn refused(code: u8) -> Self {
        Self {
            code: ExitCode::from(code),
            summary: None,
        }
    }

    /// What the batch keeps of this run for `name`; `None` when the run
    /// never started.
    fn view(&self, name: &str) -> Option<RunView> {
        let summary = self.summary.as_ref()?;
        let result = summary.results.iter().find(|result| result.name == name);
        Some(RunView {
            run_id: summary.run_id.to_string(),
            run_passed: summary.passed,
            outcome: result.map(|result| result.outcome.clone()),
            duration: result.map(|result| result.duration).unwrap_or_default(),
            cleanup_clean: summary.cleanup.as_ref().map(|report| report.is_clean()),
        })
    }
}

/// One finished run as the family report shows it.
struct RunView {
    run_id: String,
    /// The run's own verdict, which already folds in cleanup and checks.
    run_passed: bool,
    outcome: Option<mortsom_scenario_kit::outcome::Outcome>,
    cleanup_clean: Option<bool>,
    /// Wall-clock the scenario itself took, zero when it never ran.
    duration: std::time::Duration,
}

/// The outcome of a family run as a sequence of single-scenario runs.
#[derive(Default)]
struct Batch {
    lines: Vec<String>,
    passed: usize,
    failed: usize,
    skipped: usize,
    not_run: Vec<String>,
    refused: bool,
}

impl Batch {
    fn record(&mut self, name: &str, run: Option<RunView>) {
        use mortsom_scenario_kit::outcome::Outcome;
        let Some(run) = run else {
            self.refused = true;
            self.lines
                .push(format!("{name:<28} not started (see above)"));
            return;
        };
        let label = match &run.outcome {
            // A scenario that passed inside a run that did not (an order
            // left unresolved, a failed check) is not a pass of the family.
            Some(Outcome::Passed) if !run.run_passed => {
                self.failed += 1;
                "passed, but the run failed (see its checks and cleanup)".to_owned()
            }
            Some(Outcome::Passed) => {
                self.passed += 1;
                "passed".to_owned()
            }
            Some(Outcome::Skipped { reason }) => {
                self.skipped += 1;
                format!("skipped: {reason}")
            }
            Some(Outcome::Failed { category, message }) => {
                self.failed += 1;
                // One line per scenario: a driver error carries its own
                // line breaks and would push the rest of the row off it.
                format!(
                    "failed ({category}): {}",
                    message.split_whitespace().collect::<Vec<_>>().join(" ")
                )
            }
            None => {
                self.failed += 1;
                "failed: the run produced no result for it".to_owned()
            }
        };
        let cleanup = match run.cleanup_clean {
            Some(true) => "clean",
            Some(false) => "UNRESOLVED",
            None => "no cleanup report",
        };
        self.lines.push(format!(
            "{name:<28} {label} · {} · cleanup {cleanup} · run {}",
            crate::run::human_elapsed(run.duration),
            run.run_id
        ));
    }

    fn not_run<'a>(&mut self, names: impl Iterator<Item = &'a str>) {
        self.not_run.extend(names.map(str::to_owned));
    }

    fn finished(&self) -> usize {
        self.lines.len()
    }

    fn render(&self) -> String {
        let mut out = String::from("\nfamily run\n");
        for line in &self.lines {
            out.push_str(line);
            out.push('\n');
        }
        for name in &self.not_run {
            out.push_str(&format!("{name:<28} not run\n"));
        }
        out.push_str(&format!(
            "{} passed, {} failed, {} skipped, {} not run",
            self.passed,
            self.failed,
            self.skipped,
            self.not_run.len()
        ));
        out
    }

    fn code(&self) -> ExitCode {
        if self.refused {
            ExitCode::from(2)
        } else if self.failed > 0 {
            ExitCode::from(1)
        } else {
            ExitCode::SUCCESS
        }
    }
}

async fn run_once(
    config_path: &std::path::Path,
    stack_dir: &std::path::Path,
    selection: RunSelection,
    prepared: &mut super::prepared::InvocationBuild,
) -> Attempt {
    let RunSelection {
        preserve_legacy_recovery,
        scenarios,
        tag,
        platform,
        artifact,
        source,
        xvfb,
    } = selection;
    if let Err(error) = mortsom_scenario_kit::registry::select(&scenarios, tag.as_deref()) {
        eprintln!("{error}");
        return Attempt::refused(2);
    }
    let (config, stack) = match crate::desktop::load_with_source(
        config_path,
        stack_dir,
        &TokioRunner,
        &RealHost,
        source.selection().as_ref(),
    )
    .await
    {
        Ok(loaded) => loaded,
        Err(e) => {
            eprintln!("{e}");
            return Attempt::refused(2);
        }
    };
    let recovery_isolation = match super::isolation::admit(
        &preserve_legacy_recovery,
        &config,
        &stack,
        stack_dir,
    )
    .await
    {
        Ok(isolation) => isolation,
        Err(error) => {
            eprintln!("{error}");
            return Attempt::refused(2);
        }
    };
    if ![&config.wallets.alice, &config.wallets.bob]
        .iter()
        .all(|wallet| matches!(wallet, mortsom_environment::WalletConfig::Lnd { .. }))
    {
        eprintln!(
            "desktop checkpoint recovery requires Alice and Bob on caller-controlled LND wallets"
        );
        return Attempt::refused(2);
    }
    let fingerprint = match stack.fingerprint() {
        Ok(value) => value,
        Err(error) => {
            eprintln!("{error}");
            return Attempt::refused(2);
        }
    };
    let desktop = match prepared
        .get_or_prepare(fingerprint, || {
            crate::desktop::prepare(&config, platform, artifact, xvfb, &TokioRunner, &RealHost)
        })
        .await
    {
        Ok(options) => options,
        Err(error) => {
            eprintln!("{error}");
            return Attempt::refused(2);
        }
    };
    let docker = DockerCli::default();
    let adapter = select_adapter();
    let env = |name: &str| std::env::var(name).ok();
    let wallets = wallet_factory;
    let net = RealNet::with_daemon_timeout(std::time::Duration::from_secs(
        config.mostro.health_timeout_secs,
    ));
    let deps = RunDeps {
        desktop_factory: &crate::run::RealDesktopFactory,
        net: Some(&net),
        docker: Arc::new(docker),
        probe: Arc::new(NostrProbe),
        adapter,
        env: &env,
        wallets: &wallets,
        observer: &LiveObserver,
    };
    let options = RunOptions {
        recovery_isolation,
        scenarios,
        tag,
        desktop,
        stack: Some(stack),
        stack_dir: stack_dir.canonicalize().ok(),
    };
    match run_scenarios(&config, &options, &deps).await {
        Ok(summary) => {
            print!("{}", summary.checks.render_with_color(use_color()));
            print!("{}", crate::run::render_results(&summary.results));
            println!("artifacts: {}/{}", config.run.artifacts_dir, summary.run_id);
            let code = if summary.passed {
                ExitCode::SUCCESS
            } else {
                ExitCode::from(1)
            };
            Attempt {
                code,
                summary: Some(summary),
            }
        }
        Err(e) => {
            eprintln!("{e}");
            Attempt::refused(2)
        }
    }
}

/// The run's wallet factory.
///
/// A free function rather than a closure: the factory is higher-ranked over
/// the lifetime of what it borrows, and only an `fn` item names that.
pub(super) fn wallet_factory<'b>(
    label: &'b str,
    wallet: &'b mortsom_environment::WalletConfig,
) -> futures::future::BoxFuture<'b, Result<crate::run::WalletHandles, String>> {
    Box::pin(connect_wallet(label, wallet))
}

/// Connects the wallet behind one actor.
///
/// On NWC the app holds the wallet and Mortsom only reads it. On LND the
/// node is Mortsom's: it reads *and* pays, because the app under test has
/// no wallet connected at all on that path.
async fn connect_wallet(
    label: &str,
    wallet: &mortsom_environment::WalletConfig,
) -> Result<crate::run::WalletHandles, String> {
    use mortsom_environment::WalletConfig;
    match wallet {
        WalletConfig::Nwc { uri } => {
            let reader = mortsom_observers::wallet::NwcReader::new(
                label,
                uri,
                crate::run::WALLET_LOOKUP_TIMEOUT,
            )
            .map_err(|e| e.to_string())?;
            Ok(crate::run::WalletHandles {
                reader: Arc::new(reader),
                node: None,
                network: None,
            })
        }
        WalletConfig::Lnd {
            host,
            macaroon,
            cert,
        } => {
            let node = mortsom_environment::LndWallet::connect(label, host, cert, macaroon)
                .await
                .map_err(|e| e.to_string())?;
            // The node names its own network; regtest clearance rests on
            // this answer, never on what the configuration claims.
            let (_alias, network) = node.info().await.map_err(|e| e.to_string())?;
            Ok(crate::run::WalletHandles {
                reader: Arc::new(mortsom_observers::wallet::LndReader::new(node.clone())),
                node: Some(Arc::new(NodeWalletOverLnd(node))),
                network: Some(network),
            })
        }
    }
}

/// Adapts the environment's LND client to what a scenario asks of a wallet.
struct NodeWalletOverLnd(mortsom_environment::LndWallet);

#[async_trait::async_trait]
impl mortsom_scenario_kit::context::NodeWallet for NodeWalletOverLnd {
    fn routing_fee_cap_sats(&self) -> u64 {
        mortsom_environment::lnd::MAX_ROUTING_FEE_SATS as u64
    }
    async fn cancel_invoice(&self, hash: &str) -> Result<(), String> {
        self.0
            .cancel_invoice(hash)
            .await
            .map_err(|error| error.to_string())
    }
    async fn settled_record_count(&self, hash: &str, incoming: bool) -> Result<usize, String> {
        self.0
            .settled_record_count(hash, incoming)
            .await
            .map_err(|error| error.to_string())
    }
    async fn make_invoice(&self, sats: u64, memo: &str) -> Result<mortsom_common::Secret, String> {
        self.0
            .make_invoice(sats, memo)
            .await
            .map_err(|e| e.to_string())
    }
    async fn make_invoice_expiring(
        &self,
        sats: u64,
        memo: &str,
        expiry_secs: u64,
    ) -> Result<mortsom_common::Secret, String> {
        self.0
            .make_invoice_expiring(sats, memo, expiry_secs)
            .await
            .map_err(|e| e.to_string())
    }
    async fn make_hold_invoice(
        &self,
        sats: u64,
        memo: &str,
    ) -> Result<mortsom_common::Secret, String> {
        self.0
            .make_hold_invoice(sats, memo)
            .await
            .map_err(|e| e.to_string())
    }
    async fn hold_invoice_state(
        &self,
        hash: &str,
    ) -> Result<mortsom_scenario_kit::HoldInvoiceState, String> {
        use mortsom_environment::lnd::LndInvoiceState as Lnd;
        use mortsom_scenario_kit::HoldInvoiceState as Hold;
        self.0
            .invoice_state(hash)
            .await
            .map(|state| match state {
                Lnd::Open => Hold::Open,
                Lnd::Accepted => Hold::Accepted,
                Lnd::Settled => Hold::Settled,
                Lnd::Canceled => Hold::Cancelled,
            })
            .map_err(|e| e.to_string())
    }
    async fn settle_hold_invoice(&self, hash: &str) -> Result<(), String> {
        self.0
            .settle_hold_invoice(hash)
            .await
            .map_err(|e| e.to_string())
    }

    async fn start_payment(&self, invoice: &mortsom_common::Secret) -> Result<(), String> {
        self.0
            .start_payment(invoice)
            .await
            .map(|_| ())
            .map_err(|e| e.to_string())
    }
}

#[cfg(test)]
mod batch_tests {
    use super::*;
    use mortsom_common::FailureCategory;
    use mortsom_scenario_kit::outcome::Outcome;

    fn view(outcome: Option<Outcome>, clean: Option<bool>) -> RunView {
        let outcome_timed = outcome.is_some();
        RunView {
            run_id: "run-1".into(),
            run_passed: clean != Some(false),
            outcome,
            cleanup_clean: clean,
            // A run that produced no result for the scenario timed nothing.
            duration: if outcome_timed {
                std::time::Duration::from_secs(252)
            } else {
                std::time::Duration::ZERO
            },
        }
    }

    /// A family is read to find the slow cell, so every line carries the
    /// time its scenario took.
    #[test]
    fn every_family_line_reports_how_long_its_scenario_took() {
        let mut batch = Batch::default();
        batch.record("happy_sell", Some(view(Some(Outcome::Passed), Some(true))));

        let report = batch.render();

        assert!(report.contains("4m 12s"), "{report}");
    }

    /// A scenario's own pass is not the family's when its run left an
    /// order unresolved: the run's verdict wins, and the family fails.
    #[test]
    fn a_passed_scenario_in_a_run_that_did_not_pass_fails_the_family() {
        let mut batch = Batch::default();
        batch.record(
            "cancel_after_take_unpaid",
            Some(view(Some(Outcome::Passed), Some(false))),
        );
        assert_eq!(batch.code(), ExitCode::from(1));
        let report = batch.render();
        assert!(report.contains("passed, but the run failed"), "{report}");
        assert!(report.contains("cleanup UNRESOLVED"));
        assert!(report.ends_with("0 passed, 1 failed, 0 skipped, 0 not run"));
    }

    /// A family passes only when every scenario passed or was skipped with
    /// a reason, and every run cleaned up; one failure fails the family.
    #[test]
    fn a_family_fails_on_one_failed_scenario_and_passes_on_skips() {
        let mut batch = Batch::default();
        batch.record("happy_sell", Some(view(Some(Outcome::Passed), Some(true))));
        batch.record(
            "range_order",
            Some(view(
                Some(Outcome::Skipped {
                    reason: "profile lacks range-order".into(),
                }),
                Some(true),
            )),
        );
        assert_eq!(batch.code(), ExitCode::SUCCESS);
        let report = batch.render();
        assert!(
            report.contains(
                "happy_sell                   passed · 4m 12s · cleanup clean · run run-1"
            )
        );
        assert!(report.contains("skipped: profile lacks range-order"));
        assert!(report.ends_with("1 passed, 0 failed, 1 skipped, 0 not run"));

        batch.record(
            "market_price_order",
            Some(view(
                Some(Outcome::Failed {
                    category: FailureCategory::Product,
                    message: "no rate".into(),
                }),
                Some(false),
            )),
        );
        assert_eq!(batch.code(), ExitCode::from(1));
        assert!(
            batch
                .render()
                .contains("failed (product): no rate · 4m 12s · cleanup UNRESOLVED")
        );
    }

    /// A run that never started stops the family: what follows is listed
    /// as not run, and the exit code is the refusal's, not a failure's.
    #[test]
    fn a_refused_run_stops_the_family_and_lists_the_rest_as_not_run() {
        let mut batch = Batch::default();
        batch.record("happy_sell", Some(view(Some(Outcome::Passed), Some(true))));
        batch.record("happy_buy", None);
        batch.not_run(
            ["range_order", "market_price_order"]
                .into_iter()
                .skip(batch.finished() - 2),
        );
        assert_eq!(batch.code(), ExitCode::from(2));
        let report = batch.render();
        assert!(report.contains("happy_buy                    not started (see above)"));
        assert!(report.contains("range_order                  not run"));
        assert!(report.ends_with("1 passed, 0 failed, 0 skipped, 2 not run"));
    }

    /// A driver's multi-line error stays on the scenario's one line.
    #[test]
    fn a_multi_line_failure_message_is_reported_on_one_line() {
        let mut batch = Batch::default();
        batch.record(
            "happy_sell",
            Some(view(
                Some(Outcome::Failed {
                    category: FailureCategory::TestInfrastructure,
                    message: "session deleted\n  (Session info: chrome=152)".into(),
                }),
                Some(false),
            )),
        );
        let report = batch.render();
        assert!(
            report.contains(
                "session deleted (Session info: chrome=152) · 4m 12s · cleanup UNRESOLVED"
            ),
            "{report}"
        );
    }

    /// A run that ended without a result for its scenario is a failure,
    /// never silently a pass.
    #[test]
    fn a_run_without_a_result_for_its_scenario_counts_as_failed() {
        let mut batch = Batch::default();
        batch.record("happy_sell", Some(view(None, None)));
        assert_eq!(batch.code(), ExitCode::from(1));
        assert!(
            batch
                .render()
                .contains("produced no result for it · - · cleanup no cleanup report")
        );
    }
}
