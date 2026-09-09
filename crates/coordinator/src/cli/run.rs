//! Run command wiring and actor wallet connections.
use super::*;

pub(super) async fn run_command(
    config_path: &std::path::Path,
    stack_dir: &std::path::Path,
    scenarios: Vec<String>,
    tag: Option<String>,
    platform: mortsom_desktop_driver::Platform,
    artifact: Option<PathBuf>,
    xvfb: PathBuf,
) -> ExitCode {
    mortsom_scenarios::link();
    if let Err(error) = crate::run::select_for_run(
        &scenarios,
        tag.as_deref(),
        platform != mortsom_desktop_driver::Platform::Android,
    ) {
        eprintln!("{error}");
        return ExitCode::from(2);
    }
    let loaded = if platform == mortsom_desktop_driver::Platform::Android {
        Config::load(config_path)
            .map(|config| (config, None))
            .map_err(|e| e.to_string())
    } else {
        crate::desktop::load(config_path, stack_dir, &TokioRunner, &RealHost)
            .await
            .map(|(config, stack)| (config, Some(stack)))
    };
    let (config, stack) = match loaded {
        Ok(loaded) => loaded,
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::from(2);
        }
    };
    if platform != mortsom_desktop_driver::Platform::Android {
        let pending = RealHost
            .env("HOME")
            .ok_or("HOME is required for desktop recovery".to_string())
            .and_then(|home| {
                crate::recovery_store::FileJournal::directory(std::path::Path::new(&home))
            })
            .and_then(|base| {
                crate::recovery_store::FileJournal::refuse_pending(&base)
                    .map_err(|pending| pending.message(use_color_on_stderr()))
            });
        if let Err(error) = pending {
            eprintln!("{error}");
            return ExitCode::from(2);
        }
        if ![&config.wallets.alice, &config.wallets.bob]
            .iter()
            .all(|wallet| matches!(wallet, mortsom_environment::WalletConfig::Lnd { .. }))
        {
            eprintln!(
                "desktop checkpoint recovery requires Alice and Bob on caller-controlled LND wallets"
            );
            return ExitCode::from(2);
        }
    }
    let desktop = if platform == mortsom_desktop_driver::Platform::Android {
        None
    } else {
        match crate::desktop::prepare(&config, platform, artifact, xvfb, &TokioRunner, &RealHost)
            .await
        {
            Ok(options) => Some(options),
            Err(error) => {
                eprintln!("{error}");
                return ExitCode::from(2);
            }
        }
    };
    let appium = RealHost.which("appium").unwrap_or_default();
    let flutter = RealHost.which("flutter").unwrap_or_default();
    let ui = AppiumDriver::new(appium);
    let docker = DockerCli::default();
    let seed = if desktop.is_some() {
        format!("ws://127.0.0.1:{}", config.relay.host_port)
    } else {
        RelayHandle {
            container_name: String::new(),
            host_port: config.relay.host_port,
            data_dir: std::path::PathBuf::new(),
            image: String::new(),
        }
        .emulator_url()
    };
    let adapter = select_adapter(&config, flutter, seed);
    let env = |name: &str| std::env::var(name).ok();
    let wallets = wallet_factory;
    let net = RealNet::with_daemon_timeout(std::time::Duration::from_secs(
        config.mostro.health_timeout_secs,
    ));
    let deps = RunDeps {
        desktop_factory: &crate::run::RealDesktopFactory,
        net: Some(&net),
        runner: &TokioRunner,
        ui: &ui,
        docker: Arc::new(docker),
        probe: Arc::new(NostrProbe),
        adapter,
        env: &env,
        wallets: &wallets,
        observer: &LiveObserver,
    };
    let options = RunOptions {
        scenarios,
        tag,
        shutdown_grace: None,
        desktop,
        stack,
    };
    match run_scenarios(&config, &options, &deps).await {
        Ok(summary) => {
            print!("{}", summary.checks.render_with_color(use_color()));
            for result in &summary.results {
                println!(
                    "{:<28} {:?} (attempts {})",
                    result.name, result.outcome, result.attempts
                );
            }
            println!("artifacts: {}/{}", config.run.artifacts_dir, summary.run_id);
            if summary.passed {
                ExitCode::SUCCESS
            } else {
                ExitCode::from(1)
            }
        }
        Err(e) => {
            eprintln!("{e}");
            ExitCode::from(2)
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

    async fn start_payment(&self, invoice: &mortsom_common::Secret) -> Result<(), String> {
        self.0
            .start_payment(invoice)
            .await
            .map(|_| ())
            .map_err(|e| e.to_string())
    }
}
