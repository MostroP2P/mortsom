//! `mortsom run`: the scenario runner (spec §4.1, §10, Phase 4). It selects
//! scenarios, gates them on capabilities and mainnet clearance, starts the
//! relay, observers and devices, bootstraps both actors through the UI,
//! executes scenarios sequentially with timeouts and a single infra retry,
//! sweeps cleanup, and writes the joined evidence.

mod execute;
mod observer;
mod preflight;
pub(crate) mod recovery;
mod report;
mod world_execution;
pub use observer::{LiveObserver, ObserverFactory};
pub use report::RunSummary;
use report::finish;
mod desktop_factory;
pub use desktop_factory::{DesktopFactory, DesktopServer, RealDesktopFactory};
mod farm;
pub use farm::DesktopOptions;
use farm::Farm;
mod relay_control;
mod selection;
mod sink;
pub use selection::select_for_run;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures::future::BoxFuture;
use mortsom_android_driver::farm::{DeviceFarm, FarmSpec};
use mortsom_android_driver::process::CommandRunner;
use mortsom_android_driver::sdk::{AndroidSdk, avd_home};
use mortsom_android_driver::ui::UiDriver;
use mortsom_app_contract::{AppAdapter, TestEnvironment};
use mortsom_common::check::CheckStatus;
use mortsom_common::logging::attach_run_log;
use mortsom_common::{Redactor, RunId};
use mortsom_environment::WalletConfig;
use mortsom_environment::docker::ContainerRuntime;
use mortsom_environment::relay::{ReadinessProbe, RelayHandle, start_relay, teardown_relay};
use mortsom_observers::relay::RelayObserver;
use mortsom_observers::wallet::{WalletObserver, WalletReader};
use mortsom_scenario_kit::budget::{Budget, SpendCaps};
use mortsom_scenario_kit::cleanup::CleanupRegistry;
use mortsom_scenario_kit::context::NodeWallet;
use mortsom_scenario_kit::context::{ActorSession, Context, MainnetClearance, RecoveryTiming};
use mortsom_scenario_kit::outcome::{Outcome, ScenarioResult};
use mortsom_scenario_kit::recovery::{NoRelayControl, RelayControl};
use mortsom_scenario_kit::registry::{ManualPayments, ScenarioDef, required_on};
use mortsom_scenario_kit::timeline::{Timeline, TimelineKind};
use relay_control::LiveRelayControl;
use tracing::{error, info, warn};

use crate::artifacts::{RunArtifacts, files};
use crate::bootstrap::{ActorSetup, bootstrap_session};
use crate::config::Config;
use crate::evidence::StepRecorder;
use crate::safety::authorize_mainnet;

pub use execute::execute_scenario;
pub use sink::ArtifactSink;

/// Grace for emulator shutdown.
pub const SHUTDOWN_GRACE: Duration = Duration::from_secs(20);
/// Deadline for the relay observer to connect.
pub const OBSERVER_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// Deadline for wallet lookups.
pub const WALLET_LOOKUP_TIMEOUT: Duration = Duration::from_secs(20);

/// What to run.
#[derive(Debug, Clone, Default)]
pub struct RunOptions {
    /// Scenario names.
    pub scenarios: Vec<String>,
    /// Suite tag.
    pub tag: Option<String>,
    /// Optional Linux/Web execution; absent for existing Android callers.
    pub desktop: Option<DesktopOptions>,
    /// Verified public stack snapshot for persistent desktop runs.
    pub stack: Option<crate::stack::StackSnapshot>,
    /// Grace for emulator shutdown.
    pub shutdown_grace: Option<Duration>,
}

/// What a run holds for one actor's wallet.
pub struct WalletHandles {
    /// Read-only lookups, for settlement assertions.
    pub reader: Arc<dyn WalletReader>,
    /// A node Mortsom pays with, when the wallet is one it holds.
    ///
    /// `None` on the NWC path: there the app has the wallet and pays for
    /// itself, and Mortsom only watches.
    pub node: Option<Arc<dyn NodeWallet>>,
    /// The network the node itself reported at connect time (`regtest`,
    /// `mainnet`, ...), when the wallet is a node Mortsom holds. Regtest
    /// clearance rests on this answer, never on configuration.
    pub network: Option<String>,
}

/// Wallet factory (real: NWC or a local LND node; tests: fakes).
///
/// Async because connecting to a node is: the certificate is read and the
/// gRPC channel opened before the run can claim the wallet is usable.
pub type WalletFactory<'a> = dyn for<'b> Fn(&'b str, &'b WalletConfig) -> BoxFuture<'b, Result<WalletHandles, String>>
    + Send
    + Sync
    + 'a;

/// Everything the runner needs from the outside world.
pub struct RunDeps<'a> {
    /// Desktop session/server construction boundary.
    pub desktop_factory: &'a dyn DesktopFactory,
    /// Process runner.
    pub runner: &'a dyn CommandRunner,
    /// UI driver.
    pub ui: &'a dyn UiDriver,
    /// Container runtime (shared with the relay control handed to scenarios).
    pub docker: Arc<dyn ContainerRuntime>,
    /// Relay readiness probe.
    pub probe: Arc<dyn ReadinessProbe>,
    /// Application adapter.
    pub adapter: Arc<dyn AppAdapter>,
    /// Environment lookup.
    pub env: &'a dyn Fn(&str) -> Option<String>,
    /// Wallet reader factory.
    pub wallets: &'a WalletFactory<'a>,
    /// Relay observer factory.
    pub observer: &'a dyn ObserverFactory,
    /// Read-only network preflight required for Linux/Web runs.
    pub net: Option<&'a dyn crate::doctor::NetProbes>,
}

/// Runs the selected scenarios.
pub async fn run_scenarios(
    config: &Config,
    options: &RunOptions,
    deps: &RunDeps<'_>,
) -> Result<RunSummary, String> {
    if options.desktop.is_none() {
        config.require_android().map_err(|e| e.to_string())?;
    }
    let selected = if options
        .desktop
        .as_ref()
        .is_some_and(|options| options.resume)
    {
        vec![&recovery::CLEANUP]
    } else {
        select_for_run(
            &options.scenarios,
            options.tag.as_deref(),
            options.desktop.is_some(),
        )?
    };
    let clearance = clear_value_moving(config, deps, &selected)?;
    let mainnet = match clearance {
        Clearance::Mainnet(c) => Some(c),
        Clearance::NotNeeded | Clearance::PendingRegtest => None,
    };
    let caps = SpendCaps::new(config.run.max_order_sats, config.run.max_run_spend_sats)
        .map_err(|e| e.to_string())?;

    let run_id = RunId::new();
    let redactor = Redactor::new().with_secrets(config.wallets.secrets());
    let artifacts = RunArtifacts::create(
        Path::new(&config.run.artifacts_dir),
        run_id.clone(),
        redactor.clone(),
    )
    .map_err(|e| format!("cannot create artifacts: {e}"))?;
    attach_run_log(artifacts.dir(), redactor).map_err(|e| format!("cannot attach run log: {e}"))?;
    info!(run = %run_id, lane = %config.run.lane, scenarios = ?selected.iter().map(|d| d.name).collect::<Vec<_>>(), "run starting");
    let mut summary = report::initial_summary(
        config,
        options,
        run_id.clone(),
        deps.adapter.profile().name,
        mainnet.is_some(),
    )?;
    if options
        .desktop
        .as_ref()
        .is_some_and(|options| options.resume)
    {
        summary.command = "cleanup".into();
    }
    report::record_environment(&artifacts, &summary)?;
    let (runnable, gated) = gate_capabilities(
        deps.adapter.as_ref(),
        &selected,
        ManualPayments {
            alice: config.wallets.alice.drives_manual_payments(),
            bob: config.wallets.bob.drives_manual_payments(),
        },
    );
    summary.results.extend(gated);
    let (runnable, gated) = gate_solver(&summary.app_profile, runnable, options.stack.is_some());
    summary.results.extend(gated);
    if runnable.is_empty() {
        return finish(&artifacts, &mut summary).map(|()| summary);
    }

    if !preflight::check(config, options, deps, &mut summary).await {
        preflight::reject(&runnable, &mut summary);
        return finish(&artifacts, &mut summary).map(|()| summary);
    }

    report::record_environment(&artifacts, &summary)?;
    let mut options = options.clone();
    if let Err(error) = recovery::prepare(&mut options, &run_id, deps) {
        summary
            .checks
            .push("protected-recovery", CheckStatus::Fail, error);
        preflight::reject(&runnable, &mut summary);
        return finish(&artifacts, &mut summary).map(|()| summary);
    }
    let mut world = World::new(
        config,
        &options,
        deps,
        &artifacts,
        caps,
        mainnet,
        matches!(clearance, Clearance::PendingRegtest),
    );
    world.needs_solver = runnable.iter().any(|def| def.needs_solver);
    if let Err(stage) = world.bring_up(&mut summary).await {
        for def in &runnable {
            summary.results.push(ScenarioResult {
                name: def.name.into(),
                app_profile: summary.app_profile.clone(),
                app_version: summary.app_version.clone(),
                outcome: Outcome::Failed {
                    category: mortsom_common::FailureCategory::TestInfrastructure,
                    message: format!("environment did not come up: {stage}"),
                },
                attempts: 0,
                duration: Duration::ZERO,
            });
        }
        world.tear_down(&mut summary).await;
        return finish(&artifacts, &mut summary).map(|()| summary);
    }
    for def in &runnable {
        let result = world.execute(config, def, &mut summary).await;
        summary.results.push(result);
    }
    world.tear_down(&mut summary).await;
    finish(&artifacts, &mut summary).map(|()| summary)
}

/// How value-moving scenarios were cleared to run.
enum Clearance {
    /// No selected scenario moves value.
    NotNeeded,
    /// Mainnet spending was confirmed by configuration and environment.
    Mainnet(MainnetClearance),
    /// Every wallet is a node Mortsom holds; clearance is granted during
    /// bring-up, once each connected node has proved it runs on regtest.
    PendingRegtest,
}

/// Value-moving scenarios need explicit clearance before anything is
/// started (spec §16.1, Phase 4.6). Mainnet needs the double opt-in. When
/// both wallets are Mortsom-held nodes and the operator did not enable
/// mainnet, the decision is deferred to bring-up: the nodes themselves
/// must report `regtest`, where sats are worthless by construction.
fn clear_value_moving(
    config: &Config,
    deps: &RunDeps<'_>,
    selected: &[&ScenarioDef],
) -> Result<Clearance, String> {
    if !selected.iter().any(|d| d.moves_value) {
        return Ok(Clearance::NotNeeded);
    }
    let all_nodes = [&config.wallets.alice, &config.wallets.bob]
        .into_iter()
        .all(|w| matches!(w, WalletConfig::Lnd { .. }));
    if !config.run.allow_mainnet && all_nodes {
        return Ok(Clearance::PendingRegtest);
    }
    authorize_mainnet(&config.run, deps.env)
        .map(|c| Clearance::Mainnet(MainnetClearance { lane: c.lane }))
        .map_err(|e| e.to_string())
}

/// Splits scenarios into runnable and skipped-with-reason (spec §12.2).
fn gate_capabilities<'a>(
    adapter: &dyn AppAdapter,
    selected: &[&'a ScenarioDef],
    manual: ManualPayments,
) -> (Vec<&'a ScenarioDef>, Vec<ScenarioResult>) {
    let capabilities = adapter.capabilities();
    let profile = adapter.profile().name;
    let mut runnable = Vec::new();
    let mut skipped = Vec::new();
    let mut skip = |def: &ScenarioDef, reason: String| {
        skipped.push(ScenarioResult {
            name: def.name.into(),
            app_profile: profile.clone(),
            app_version: None,
            outcome: Outcome::Skipped { reason },
            attempts: 0,
            duration: Duration::ZERO,
        });
    };
    for def in selected {
        // A scenario about the app's own wallet cannot be run against one
        // Mortsom holds: the path it exists to exercise is not taken, and
        // reporting a pass for it would say the suite covered something it
        // never touched.
        if def.needs_app_wallet && manual.alice {
            skip(
                def,
                format!(
                    "{} is about the wallet the app is connected to, and this run pays \
                     alice's side from a node Mortsom holds",
                    def.name
                ),
            );
            continue;
        }
        // What is left of a scenario's requirements depends on the wallet
        // each actor was given: where Mortsom pays, the app's own wallet is
        // never reached and the manual controls are what must exist.
        let missing = capabilities.missing(&required_on(def.requires, manual));
        if missing.is_empty() {
            runnable.push(*def);
        } else {
            let reason = format!(
                "profile {profile} lacks {}",
                missing
                    .iter()
                    .map(|c| c.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            );
            skip(def, reason);
        }
    }
    (runnable, skipped)
}

/// A dispute solver is a user the daemon's operator registers with the
/// daemon's own key, which only a Mortsom-owned stack holds: elsewhere a
/// scenario that needs one is skipped with the reason, never failed.
fn gate_solver<'a>(
    profile: &str,
    selected: Vec<&'a ScenarioDef>,
    stack_owned: bool,
) -> (Vec<&'a ScenarioDef>, Vec<ScenarioResult>) {
    let mut runnable = Vec::new();
    let mut skipped = Vec::new();
    for def in selected {
        if def.needs_solver && !stack_owned {
            skipped.push(ScenarioResult {
                name: def.name.into(),
                app_profile: profile.to_owned(),
                app_version: None,
                outcome: Outcome::Skipped {
                    reason: format!(
                        "{} needs a dispute solver, which only a Mortsom-owned stack can register \
                         (`mortsom stack up`)",
                        def.name
                    ),
                },
                attempts: 0,
                duration: Duration::ZERO,
            });
        } else {
            runnable.push(def);
        }
    }
    (runnable, skipped)
}

/// The live environment of one run.
struct World<'a> {
    config: &'a Config,
    deps: &'a RunDeps<'a>,
    artifacts: &'a RunArtifacts,
    shutdown_grace: Duration,
    budget: Arc<Mutex<Budget>>,
    mainnet: Option<MainnetClearance>,
    /// Value clearance still owed: granted in `bring_up` once every node
    /// wallet has proved it runs on regtest.
    regtest_pending: bool,
    cleanup: Arc<CleanupRegistry>,
    timeline: Arc<Timeline>,
    recorder: Arc<StepRecorder>,
    relay: Option<RelayHandle>,
    observer: Option<Arc<RelayObserver>>,
    wallets: Option<Arc<WalletObserver>>,
    node_wallets: BTreeMap<String, Arc<dyn NodeWallet>>,
    farm: Option<Farm>,
    desktop: Option<DesktopOptions>,
    /// Whether a scenario of this run needs a dispute solver.
    needs_solver: bool,
    /// The solver registered for this run, when one was needed.
    solver: Option<Arc<mortsom_solver::Solver>>,
}

impl<'a> World<'a> {
    fn new(
        config: &'a Config,
        options: &RunOptions,
        deps: &'a RunDeps<'a>,
        artifacts: &'a RunArtifacts,
        caps: SpendCaps,
        mainnet: Option<MainnetClearance>,
        regtest_pending: bool,
    ) -> Self {
        Self {
            config,
            deps,
            artifacts,
            shutdown_grace: options.shutdown_grace.unwrap_or(SHUTDOWN_GRACE),
            budget: Arc::new(Mutex::new(Budget::new(caps))),
            mainnet,
            regtest_pending,
            cleanup: Arc::new(CleanupRegistry::default()),
            timeline: Arc::new(Timeline::default()),
            recorder: Arc::new(StepRecorder::default()),
            relay: None,
            observer: None,
            wallets: None,
            node_wallets: BTreeMap::new(),
            farm: None,
            desktop: options.desktop.clone(),
            needs_solver: false,
            solver: None,
        }
    }

    /// Registers a fresh solver with the daemon, as its operator would,
    /// and scopes the observer to it. The daemon key is read from the
    /// stack's owner-only settings and dropped once the registration is
    /// signed; it is never logged or kept.
    async fn register_solver(
        &self,
        relay_url: &str,
        daemon: nostr_sdk::prelude::PublicKey,
    ) -> Result<Arc<mortsom_solver::Solver>, String> {
        let settings = mortsom_environment::daemon_settings::DaemonSettings::load(Path::new(
            &self.config.mostro.settings_file,
        ))
        .map_err(|e| e.to_string())?;
        let nsec = settings
            .nostr
            .nsec_privkey
            .filter(|s| !s.expose().is_empty())
            .ok_or_else(|| {
                format!(
                    "{} carries no daemon key; a solver can only be registered on a Mortsom-owned stack",
                    self.config.mostro.settings_file
                )
            })?;
        let solver = mortsom_solver::Solver::connect(
            relay_url,
            daemon,
            Duration::from_secs(self.config.run.state_timeout_secs),
        )
        .await
        .map_err(|e| e.to_string())?;
        solver.register(&nsec).await.map_err(|e| e.to_string())?;
        if let Some(observer) = &self.observer
            && let Err(e) = observer.add_actor(solver.public_key()).await
        {
            warn!(error = %e, "cannot scope observer to the solver");
        }
        Ok(Arc::new(solver))
    }

    /// Relay, observer, wallets, devices, bootstrap. Returns the failing
    /// stage name on error; whatever started is torn down by `tear_down`.
    async fn bring_up(&mut self, summary: &mut RunSummary) -> Result<(), String> {
        let relay_timeout = Duration::from_secs(self.config.mostro.health_timeout_secs);
        let relay = if self.desktop.is_some() {
            let handle = RelayHandle {
                container_name: String::new(),
                host_port: self.config.relay.host_port,
                data_dir: PathBuf::new(),
                image: String::new(),
            };
            if !self.deps.probe.is_ready(&handle.host_url()).await {
                return Err(record_fail(
                    summary,
                    "stack-relay",
                    "persistent relay is not ready; run `mortsom stack up`".into(),
                ));
            }
            handle
        } else {
            let relay = start_relay(
                self.deps.docker.as_ref(),
                self.deps.probe.as_ref(),
                &self.config.relay,
                summary.run_id.as_str(),
                self.artifacts.dir(),
                relay_timeout,
            )
            .await
            .map_err(|e| record_fail(summary, "relay-start", e.to_string()))?;
            summary
                .checks
                .push("relay-start", CheckStatus::Pass, relay.host_url());
            summary.relay = Some(relay.clone());
            self.relay = Some(relay.clone());
            relay
        };

        let pubkey =
            self.config.mobile.mostro_pubkey().ok_or_else(|| {
                record_fail(summary, "mostro-pubkey", "invalid daemon pubkey".into())
            })?;
        let observer = self
            .deps
            .observer
            .start(&relay.host_url(), pubkey)
            .await
            .map_err(|e| record_fail(summary, "relay-observer", e))?;
        summary
            .checks
            .push("relay-observer", CheckStatus::Pass, "subscribed");
        self.observer = Some(observer);
        if self.needs_solver {
            let solver = self
                .register_solver(&relay.host_url(), pubkey)
                .await
                .map_err(|e| record_fail(summary, "dispute-solver", e))?;
            summary.checks.push(
                "dispute-solver",
                CheckStatus::Pass,
                format!("registered {}", short(&solver.public_key().to_hex())),
            );
            self.solver = Some(solver);
        }

        let mut readers = Vec::new();
        let mut nodes: BTreeMap<String, Arc<dyn NodeWallet>> = BTreeMap::new();
        let mut networks: Vec<(String, String)> = Vec::new();
        for (label, wallet) in [
            ("alice", &self.config.wallets.alice),
            ("bob", &self.config.wallets.bob),
        ] {
            let handles = (self.deps.wallets)(label, wallet)
                .await
                .map_err(|e| record_fail(summary, "wallet-observer", e))?;
            if let Some(node) = handles.node {
                nodes.insert(label.to_owned(), node);
                // A node that answered without naming its network cannot
                // clear the regtest path; record it as such and let
                // `authorize_regtest` refuse it by name.
                networks.push((
                    label.to_owned(),
                    handles.network.unwrap_or_else(|| "unknown".to_owned()),
                ));
            }
            readers.push((label.to_owned(), handles.reader));
        }
        self.node_wallets = nodes;
        summary.checks.push(
            "wallet-observer",
            CheckStatus::Pass,
            "alice and bob readers ready",
        );
        self.wallets = Some(Arc::new(WalletObserver::new(readers)));
        if self.regtest_pending {
            match crate::safety::authorize_regtest(&self.config.run, &networks) {
                Ok(clearance) => {
                    self.mainnet = Some(MainnetClearance {
                        lane: clearance.lane,
                    });
                    summary.checks.push(
                        "regtest-clearance",
                        CheckStatus::Pass,
                        "every node reports regtest; value moves without the mainnet guard",
                    );
                }
                Err(e) => return Err(record_fail(summary, "regtest-clearance", e.to_string())),
            }
        }

        let farm = self.bring_up_farm(summary).await?;
        summary.app_version = farm.version();
        let env = self.env();
        for (actor, session) in farm.sessions() {
            let ctx = mortsom_app_contract::DeviceContext {
                actor,
                ui: session.as_ref(),
                ui_timeout: Duration::from_secs(self.config.ui_timeout_secs()),
            };
            let actor =
                if let Some(options) = self.desktop.as_ref().filter(|options| options.resume) {
                    let result = recovery::resume_actor(
                        self,
                        &ctx,
                        options.recovery.as_ref().ok_or("resume journal missing")?,
                    )
                    .await;
                    match result {
                        Ok(actor) => actor,
                        Err(error) => {
                            self.farm = Some(farm);
                            return Err(record_fail(summary, "recovery-identity", error));
                        }
                    }
                } else {
                    bootstrap_session(
                        self.config,
                        self.deps.adapter.as_ref(),
                        ctx,
                        &env,
                        self.artifacts,
                        &self.recorder,
                        &mut summary.checks,
                    )
                    .await
                };
            self.observe_actor(&actor).await;
            summary.actors.push(actor);
        }
        if let Some(journal) = self
            .desktop
            .as_ref()
            .and_then(|options| options.recovery.as_ref())
        {
            let identities = summary
                .actors
                .iter()
                .filter_map(|actor| {
                    actor
                        .identity
                        .as_ref()
                        .map(|identity| (actor.actor.clone(), identity.public_key.clone()))
                })
                .collect();
            let result = journal.set_actors(identities, farm.origin());
            self.farm = Some(farm);
            result.map_err(|error| record_fail(summary, "recovery-identity", error))?;
        } else {
            self.farm = Some(farm);
        }
        // On the LND path the app is deliberately walletless, so a missing
        // wallet is the intended, successful outcome there.
        let bootstrapped = summary.actors.iter().all(|a| {
            a.identity.is_some()
                && (!a.wallet_expected || a.wallet.as_ref().is_some_and(|w| w.connected))
        });
        let identities_restored = summary.actors.iter().all(|actor| {
            actor
                .identity
                .as_ref()
                .zip(actor.identity_after_restart.as_ref())
                .is_some_and(|(before, after)| before.public_key == after.public_key)
        });
        let distinct = summary.actors.len() == 2
            && summary.actors[0]
                .identity
                .as_ref()
                .zip(summary.actors[1].identity.as_ref())
                .is_some_and(|(alice, bob)| alice.public_key != bob.public_key);
        if !identities_restored || !distinct || !summary.checks.is_healthy() {
            return Err(record_fail(
                summary,
                "actor-isolation",
                "bootstrap checks, distinct identities and identity persistence must all pass"
                    .into(),
            ));
        }
        if !bootstrapped {
            return Err("actor bootstrap failed (see checks)".into());
        }
        Ok(())
    }

    async fn bring_up_farm(&self, summary: &mut RunSummary) -> Result<Farm, String> {
        if let Some(options) = &self.desktop {
            return Farm::desktop(options, self.deps.desktop_factory).await;
        }
        let apk = resolve_apk(self.config, self.deps)
            .await
            .map_err(|e| record_fail(summary, "apk", e))?;
        summary
            .checks
            .push("apk", CheckStatus::Pass, apk.display().to_string());
        let avd_home = avd_home(self.deps.env).ok_or_else(|| {
            record_fail(summary, "avd-home", "cannot resolve the AVD home".into())
        })?;
        let sdk = AndroidSdk::locate(self.deps.env, |p| p.exists())
            .map_err(|e| record_fail(summary, "android-sdk", e.to_string()))?;
        let spec = FarmSpec {
            android: self
                .config
                .require_android()
                .map_err(|e| e.to_string())?
                .clone(),
            apk,
            package: self.config.mobile.package.clone(),
            main_activity: (!self.config.mobile.main_activity.is_empty())
                .then(|| self.config.mobile.main_activity.clone()),
            avd_home,
            log_dir: self.artifacts.dir().to_path_buf(),
            shutdown_grace: self.shutdown_grace,
        };
        let farm = DeviceFarm::bring_up(self.deps.runner, self.deps.ui, &sdk, &spec)
            .await
            .map_err(|e| record_fail(summary, "device-farm", e.to_string()))?;
        summary.checks.push(
            "device-farm",
            CheckStatus::Pass,
            "alice and bob booted, same build installed",
        );
        Ok(Farm::Android(farm))
    }

    /// Widens the observer scope to the actor's displayed identity so its
    /// direct messages are logged as metadata (spec §4.3). An unparsable or
    /// missing identity is already a failed bootstrap check; it is only
    /// noted here.
    async fn observe_actor(&self, actor: &ActorSetup) {
        let Some(observer) = &self.observer else {
            return;
        };
        let Some(identity) = &actor.identity else {
            return;
        };
        match nostr_sdk::prelude::PublicKey::parse(&identity.public_key) {
            Ok(pubkey) => {
                if let Err(e) = observer.add_actor(pubkey).await {
                    warn!(actor = %actor.actor, error = %e, "cannot scope observer to actor");
                }
            }
            Err(e) => warn!(actor = %actor.actor, error = %e, "actor identity is not a pubkey"),
        }
    }

    /// Fault injection over the run relay; refuses when no relay is up.
    fn relay_control(&self) -> Arc<dyn RelayControl> {
        match &self.relay {
            Some(handle) => Arc::new(LiveRelayControl::new(
                Arc::clone(&self.deps.docker),
                Arc::clone(&self.deps.probe),
                handle.clone(),
                Duration::from_secs(self.config.mostro.health_timeout_secs),
            )),
            None => Arc::new(NoRelayControl),
        }
    }

    fn env(&self) -> TestEnvironment {
        TestEnvironment {
            mostro_pubkey: self.config.mobile.mostro_pubkey.clone(),
            relay_url: if self.desktop.is_some() {
                format!("ws://127.0.0.1:{}", self.config.relay.host_port)
            } else {
                self.relay
                    .as_ref()
                    .map(RelayHandle::emulator_url)
                    .unwrap_or_default()
            },
            lane: self.config.run.lane,
        }
    }
}

fn record_fail(summary: &mut RunSummary, name: &str, detail: String) -> String {
    summary.checks.push(name, CheckStatus::Fail, detail.clone());
    format!("{name}: {detail}")
}

async fn resolve_apk(config: &Config, deps: &RunDeps<'_>) -> Result<PathBuf, String> {
    crate::apk::resolve_apk(config, deps.adapter.as_ref(), deps.runner).await
}

fn short(id: &str) -> String {
    id.chars().take(12).collect()
}

#[cfg(test)]
mod tests;
