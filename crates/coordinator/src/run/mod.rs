//! `mortsom run`: the scenario runner (spec §4.1, §10, Phase 4). It selects
//! scenarios, gates them on capabilities and mainnet clearance, starts the
//! relay, observers and devices, bootstraps both actors through the UI,
//! executes scenarios sequentially with timeouts and a single infra retry,
//! sweeps cleanup, and writes the joined evidence.

mod execute;
mod relay_control;
mod sink;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures::future::BoxFuture;
use mortsom_android_driver::farm::{DeviceFarm, FarmSpec, TeardownItem};
use mortsom_android_driver::process::CommandRunner;
use mortsom_android_driver::sdk::{AndroidSdk, avd_home};
use mortsom_android_driver::ui::UiDriver;
use mortsom_app_contract::{AppAdapter, TestEnvironment};
use mortsom_common::check::{CheckStatus, DoctorReport};
use mortsom_common::logging::attach_run_log;
use mortsom_common::{Lane, Redactor, RunId};
use mortsom_environment::WalletConfig;
use mortsom_environment::docker::ContainerRuntime;
use mortsom_environment::relay::{
    ReadinessProbe, RelayHandle, RelayTeardown, start_relay, teardown_relay,
};
use mortsom_observers::relay::{ObserverScope, RelayObserver};
use mortsom_observers::wallet::{WalletObserver, WalletReader};
use mortsom_scenario_kit::budget::{Budget, SpendCaps, Totals};
use mortsom_scenario_kit::cleanup::{CleanupRegistry, CleanupReport};
use mortsom_scenario_kit::context::NodeWallet;
use mortsom_scenario_kit::context::{ActorSession, Context, MainnetClearance, RecoveryTiming};
use mortsom_scenario_kit::outcome::{Outcome, ScenarioResult};
use mortsom_scenario_kit::recovery::{NoRelayControl, RelayControl};
use mortsom_scenario_kit::registry::{ScenarioDef, select};
use mortsom_scenario_kit::timeline::{Timeline, TimelineKind};
use relay_control::LiveRelayControl;
use serde::{Deserialize, Serialize};
use tracing::{error, info, warn};

use crate::artifacts::{RunArtifacts, files};
use crate::bootstrap::{ActorSetup, bootstrap_actor};
use crate::config::Config;
use crate::evidence::StepRecorder;
use crate::report::{RunReport, write_report};
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
}

/// Wallet factory (real: NWC or a local LND node; tests: fakes).
///
/// Async because connecting to a node is: the certificate is read and the
/// gRPC channel opened before the run can claim the wallet is usable.
pub type WalletFactory<'a> = dyn for<'b> Fn(&'b str, &'b WalletConfig) -> BoxFuture<'b, Result<WalletHandles, String>>
    + Send
    + Sync
    + 'a;

/// Starts the relay observer for a run (real: connects; tests: detached).
#[async_trait::async_trait]
pub trait ObserverFactory: Send + Sync {
    /// Starts observing `relay_url` for the daemon `pubkey`.
    async fn start(
        &self,
        relay_url: &str,
        pubkey: nostr_sdk::prelude::PublicKey,
    ) -> Result<Arc<RelayObserver>, String>;
}

/// Connects to the run relay.
#[derive(Debug, Default, Clone, Copy)]
pub struct LiveObserver;

#[async_trait::async_trait]
impl ObserverFactory for LiveObserver {
    async fn start(
        &self,
        relay_url: &str,
        pubkey: nostr_sdk::prelude::PublicKey,
    ) -> Result<Arc<RelayObserver>, String> {
        RelayObserver::start(
            &[relay_url.to_owned()],
            ObserverScope::daemon_only(pubkey),
            OBSERVER_CONNECT_TIMEOUT,
        )
        .await
        .map(Arc::new)
        .map_err(|e| e.to_string())
    }
}

/// Everything the runner needs from the outside world.
pub struct RunDeps<'a> {
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
}

/// Machine-readable outcome, written to `summary.json`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunSummary {
    /// Run identifier.
    pub run_id: RunId,
    /// Lane.
    pub lane: Lane,
    /// Command.
    pub command: String,
    /// App profile.
    pub app_profile: String,
    /// App version, when known.
    pub app_version: Option<String>,
    /// Whether mainnet clearance was granted.
    pub mainnet_cleared: bool,
    /// Relay used.
    pub relay: Option<RelayHandle>,
    /// Actor bootstrap outcomes.
    pub actors: Vec<ActorSetup>,
    /// Scenario results.
    pub results: Vec<ScenarioResult>,
    /// Environment and bootstrap checks.
    pub checks: DoctorReport,
    /// Cleanup report.
    pub cleanup: Option<CleanupReport>,
    /// Budget totals.
    pub budget: Option<Totals>,
    /// Farm teardown.
    pub farm_teardown: Vec<TeardownItem>,
    /// Relay teardown.
    pub relay_teardown: Option<RelayTeardown>,
    /// True when every check passed and every selected scenario passed or
    /// was skipped with a reason.
    pub passed: bool,
}

/// Runs the selected scenarios.
pub async fn run_scenarios(
    config: &Config,
    options: &RunOptions,
    deps: &RunDeps<'_>,
) -> Result<RunSummary, String> {
    let selected = select(&options.scenarios, options.tag.as_deref())?;
    let mainnet = clear_mainnet(config, deps, &selected)?;
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
    let mut summary = RunSummary {
        run_id: run_id.clone(),
        lane: config.run.lane,
        command: "run".into(),
        app_profile: deps.adapter.profile().name,
        app_version: None,
        mainnet_cleared: mainnet.is_some(),
        relay: None,
        actors: Vec::new(),
        results: Vec::new(),
        checks: DoctorReport::default(),
        cleanup: None,
        budget: None,
        farm_teardown: Vec::new(),
        relay_teardown: None,
        passed: false,
    };
    let (runnable, gated) = gate_capabilities(deps.adapter.as_ref(), &selected);
    summary.results.extend(gated);
    if runnable.is_empty() {
        return finish(&artifacts, &mut summary).map(|()| summary);
    }

    let mut world = World::new(config, options, deps, &artifacts, caps, mainnet);
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

/// Value-moving scenarios need explicit mainnet clearance before anything
/// is started (spec §16.1, Phase 4.6).
fn clear_mainnet(
    config: &Config,
    deps: &RunDeps<'_>,
    selected: &[&ScenarioDef],
) -> Result<Option<MainnetClearance>, String> {
    if !selected.iter().any(|d| d.moves_value) {
        return Ok(None);
    }
    authorize_mainnet(&config.run, deps.env)
        .map(|c| Some(MainnetClearance { lane: c.lane }))
        .map_err(|e| e.to_string())
}

/// Splits scenarios into runnable and skipped-with-reason (spec §12.2).
fn gate_capabilities<'a>(
    adapter: &dyn AppAdapter,
    selected: &[&'a ScenarioDef],
) -> (Vec<&'a ScenarioDef>, Vec<ScenarioResult>) {
    let capabilities = adapter.capabilities();
    let profile = adapter.profile().name;
    let mut runnable = Vec::new();
    let mut skipped = Vec::new();
    for def in selected {
        let missing = capabilities.missing(def.requires);
        if missing.is_empty() {
            runnable.push(*def);
        } else {
            skipped.push(ScenarioResult {
                name: def.name.into(),
                app_profile: profile.clone(),
                app_version: None,
                outcome: Outcome::Skipped {
                    reason: format!(
                        "profile {profile} lacks {}",
                        missing
                            .iter()
                            .map(|c| c.as_str())
                            .collect::<Vec<_>>()
                            .join(", ")
                    ),
                },
                attempts: 0,
                duration: Duration::ZERO,
            });
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
    cleanup: Arc<CleanupRegistry>,
    timeline: Arc<Timeline>,
    recorder: Arc<StepRecorder>,
    relay: Option<RelayHandle>,
    observer: Option<Arc<RelayObserver>>,
    wallets: Option<Arc<WalletObserver>>,
    node_wallets: BTreeMap<String, Arc<dyn NodeWallet>>,
    farm: Option<DeviceFarm>,
}

impl<'a> World<'a> {
    fn new(
        config: &'a Config,
        options: &RunOptions,
        deps: &'a RunDeps<'a>,
        artifacts: &'a RunArtifacts,
        caps: SpendCaps,
        mainnet: Option<MainnetClearance>,
    ) -> Self {
        Self {
            config,
            deps,
            artifacts,
            shutdown_grace: options.shutdown_grace.unwrap_or(SHUTDOWN_GRACE),
            budget: Arc::new(Mutex::new(Budget::new(caps))),
            mainnet,
            cleanup: Arc::new(CleanupRegistry::default()),
            timeline: Arc::new(Timeline::default()),
            recorder: Arc::new(StepRecorder::default()),
            relay: None,
            observer: None,
            wallets: None,
            node_wallets: BTreeMap::new(),
            farm: None,
        }
    }

    /// Relay, observer, wallets, devices, bootstrap. Returns the failing
    /// stage name on error; whatever started is torn down by `tear_down`.
    async fn bring_up(&mut self, summary: &mut RunSummary) -> Result<(), String> {
        let relay_timeout = Duration::from_secs(self.config.mostro.health_timeout_secs);
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

        let mut readers = Vec::new();
        let mut nodes: BTreeMap<String, Arc<dyn NodeWallet>> = BTreeMap::new();
        for (label, wallet) in [
            ("alice", &self.config.wallets.alice),
            ("bob", &self.config.wallets.bob),
        ] {
            let handles = (self.deps.wallets)(label, wallet)
                .await
                .map_err(|e| record_fail(summary, "wallet-observer", e))?;
            if let Some(node) = handles.node {
                nodes.insert(label.to_owned(), node);
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

        let farm = self.bring_up_farm(summary).await?;
        summary.app_version = Some(
            farm.device(mortsom_android_driver::avd::Actor::Alice)
                .version
                .version_name
                .clone(),
        );
        let env = self.env();
        for device in farm.devices() {
            let actor = bootstrap_actor(
                self.config,
                self.deps.adapter.as_ref(),
                device,
                &env,
                self.artifacts,
                &self.recorder,
                &mut summary.checks,
            )
            .await;
            self.observe_actor(&actor).await;
            summary.actors.push(actor);
        }
        self.farm = Some(farm);
        let bootstrapped = summary
            .actors
            .iter()
            .all(|a| a.identity.is_some() && a.wallet.as_ref().is_some_and(|w| w.connected));
        if !bootstrapped {
            return Err("actor bootstrap failed (see checks)".into());
        }
        Ok(())
    }

    async fn bring_up_farm(&self, summary: &mut RunSummary) -> Result<DeviceFarm, String> {
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
            android: self.config.android.clone(),
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
        Ok(farm)
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
            relay_url: self
                .relay
                .as_ref()
                .map(RelayHandle::emulator_url)
                .unwrap_or_default(),
            lane: self.config.run.lane,
        }
    }

    /// Runs one scenario (with the retry policy) inside the live world.
    async fn execute(
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
            .devices()
            .iter()
            .map(|d| ActorSession {
                actor: d.actor,
                ui: Arc::clone(&d.session),
                wallet_label: d.actor.as_str().to_owned(),
            })
            .collect();
        let ctx = Context {
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
            mainnet: self.mainnet,
            amount_sats: config.run.amount_sats,
            fiat_code: config.run.fiat_code.clone(),
            fiat_amount: config.run.fiat_amount,
            payment_method: config.run.payment_method.clone(),
            ui_timeout: Duration::from_secs(config.android.ui_timeout_secs),
            state_timeout: Duration::from_secs(config.run.state_timeout_secs),
            settle_timeout: Duration::from_secs(config.run.settle_timeout_secs),
            expiration_timeout: Duration::from_secs(config.run.expiration_timeout_secs),
            recovery: RecoveryTiming {
                quiet_window: Duration::from_secs(config.run.quiet_window_secs),
                background_secs: config.run.background_secs,
                outage: Duration::from_secs(config.run.outage_secs),
            },
        };
        let result = execute_scenario(
            def,
            &ctx,
            config.run.infra_retries,
            &summary.app_profile,
            summary.app_version.clone(),
        )
        .await;
        execute::sweep_cleanup(&ctx).await;
        result
    }

    /// Cleanup report, teardown of devices and relay, evidence flush.
    async fn tear_down(&mut self, summary: &mut RunSummary) {
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
                    error!(error = %e, "cannot append relay event");
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
                error!(error = %e, "cannot write wallet payments");
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
                error!(error = %e, "cannot write relay log");
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
            error!(error = %e, "cannot write timeline");
        }
        if let Err(e) = self
            .artifacts
            .write_json(files::TIMELINE_JSON, &self.timeline.entries())
        {
            error!(error = %e, "cannot write timeline.json");
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

fn finish(artifacts: &RunArtifacts, summary: &mut RunSummary) -> Result<(), String> {
    let scenarios_ok = summary
        .results
        .iter()
        .all(|r| !matches!(r.outcome, Outcome::Failed { .. }));
    let clean = summary.cleanup.as_ref().is_none_or(CleanupReport::is_clean);
    summary.passed = summary.checks.is_healthy() && scenarios_ok && clean;
    let cleanup = serde_json::json!({
        "orders": summary.cleanup,
        "farm": summary.farm_teardown,
        "relay": summary.relay_teardown,
    });
    let environment = serde_json::json!({
        "run_id": summary.run_id,
        "lane": summary.lane,
        "command": summary.command,
        "app_profile": summary.app_profile,
        "app_version": summary.app_version,
        "mainnet_cleared": summary.mainnet_cleared,
        "relay": summary.relay.as_ref().map(|r| serde_json::json!({"container_name": r.container_name, "host_port": r.host_port})),
    });
    // The timeline is the scenario timeline written at teardown; the report
    // writer's table would overwrite it, so a run keeps its own file and the
    // report only writes summary/environment/cleanup.
    let report = RunReport {
        run_id: &summary.run_id,
        lane: summary.lane,
        command: &summary.command,
        app_profile: Some(&summary.app_profile),
        checks: &summary.checks,
        passed: summary.passed,
    };
    let timeline_exists = artifacts
        .file(files::TIMELINE)
        .map(|p| p.is_file())
        .unwrap_or(false);
    write_report(
        artifacts,
        &report,
        summary,
        &environment,
        &cleanup,
        !timeline_exists,
    )
}

#[cfg(test)]
mod tests;
