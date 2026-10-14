//! Run command wiring and actor wallet connections.
use super::run_report::{self, Footer, Report, RunView};
use super::*;
use crate::style::Style;
use mortsom_common::check::CheckStatus;

pub(super) async fn run_command(
    config_path: &std::path::Path,
    stack_dir: &std::path::Path,
    selection: RunSelection,
) -> ExitCode {
    let started = std::time::Instant::now();
    mortsom_scenarios::link();
    let selected = match mortsom_scenario_kit::registry::validate().and_then(|()| {
        mortsom_scenario_kit::registry::select(&selection.scenarios, selection.tag.as_deref())
    }) {
        Ok(selected) => selected,
        Err(error) => {
            eprintln!("{error}");
            return ExitCode::from(2);
        }
    };
    let progress = Style::new(use_color_on_stderr());
    let moving = selected.iter().filter(|def| def.moves_value).count();
    eprintln!("{}", run_report::header(selected.len(), moving, progress));
    // A run starts fresh actors and resolves everything it created before
    // it ends, so a selection is a sequence of such runs, one scenario each,
    // and never one run sharing Alice and Bob across scenarios.
    let mut prepared = super::prepared::InvocationBuild::default();
    let mut report = Report::default();
    let mut artifacts_dir = None;
    for (index, def) in selected.iter().enumerate() {
        if selected.len() > 1 {
            let line = run_report::progress(index + 1, selected.len(), def.name, progress);
            eprintln!("{line}");
        }
        let one = selection.for_scenario(def.name);
        match run_once(config_path, stack_dir, one, &mut prepared).await {
            Attempt::Ran { summary, artifacts } => {
                artifacts_dir.get_or_insert(artifacts);
                report.record(def.name, view(&summary, def.name));
            }
            Attempt::Refused(reason) => {
                // The refusal (a retained journal, a missing stack) holds for
                // the rest of the selection as well.
                report.refuse(def.name, &reason);
                report.not_run(selected.iter().skip(report.finished()).map(|d| d.name));
                break;
            }
        }
    }
    let footer = Footer {
        elapsed: started.elapsed(),
        artifacts_dir: artifacts_dir.as_deref(),
    };
    print!("\n{}", report.render(&footer, Style::new(use_color())));
    report.code()
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

impl RunSelection {
    /// The same invocation narrowed to the one scenario a run executes.
    fn for_scenario(&self, name: &str) -> Self {
        Self {
            preserve_legacy_recovery: self.preserve_legacy_recovery.clone(),
            scenarios: vec![name.to_owned()],
            tag: None,
            platform: self.platform,
            artifact: self.artifact.clone(),
            xvfb: self.xvfb.clone(),
            source: self.source.clone(),
        }
    }
}

/// How one run ended.
enum Attempt {
    /// It started and ended; its summary and the artifacts directory it
    /// wrote under.
    Ran {
        summary: Box<crate::run::RunSummary>,
        artifacts: String,
    },
    /// The environment refused to start it, and why.
    Refused(String),
}

/// What the report keeps of a finished run for scenario `name`.
fn view(summary: &crate::run::RunSummary, name: &str) -> RunView {
    let result = summary.results.iter().find(|result| result.name == name);
    RunView {
        run_id: summary.run_id.to_string(),
        run_passed: summary.passed,
        outcome: result.map(|result| result.outcome.clone()),
        attempts: result.map_or(0, |result| result.attempts),
        duration: result.map(|result| result.duration).unwrap_or_default(),
        failed_checks: summary
            .checks
            .checks
            .iter()
            .filter(|check| check.status == CheckStatus::Fail)
            .map(|check| format!("{}: {}", check.name, check.detail))
            .collect(),
        unresolved: summary
            .cleanup
            .as_ref()
            .map(|cleanup| cleanup.unresolved.clone())
            .unwrap_or_default(),
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
            return Attempt::Refused(e.to_string());
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
            return Attempt::Refused(error.to_string());
        }
    };
    if ![&config.wallets.alice, &config.wallets.bob]
        .iter()
        .all(|wallet| matches!(wallet, mortsom_environment::WalletConfig::Lnd { .. }))
    {
        return Attempt::Refused(
            "desktop checkpoint recovery requires Alice and Bob on caller-controlled LND wallets"
                .to_owned(),
        );
    }
    let fingerprint = match stack.fingerprint() {
        Ok(value) => value,
        Err(error) => {
            return Attempt::Refused(error.to_string());
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
            return Attempt::Refused(error.to_string());
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
        Ok(summary) => Attempt::Ran {
            summary: Box::new(summary),
            artifacts: config.run.artifacts_dir.clone(),
        },
        Err(error) => Attempt::Refused(error),
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
