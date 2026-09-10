//! Per-actor session bootstrap of the scenario runner: launch clean,
//! onboard, configure node and relay, connect the wallet if the app has one,
//! prove the identity survives a restart. Each step records evidence and a
//! check.

use mortsom_app_contract::{
    AppAdapter, DeviceContext, Identity, TestEnvironment, WalletConnection,
};
use mortsom_common::Actor;
use mortsom_common::check::{CheckStatus, DoctorReport};
use mortsom_environment::WalletConfig;
use serde::{Deserialize, Serialize};

use crate::artifacts::RunArtifacts;
use crate::config::Config;
use crate::evidence::StepRecorder;

/// Per-actor outcome.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActorSetup {
    /// Actor name.
    pub actor: String,
    /// Identity as displayed after onboarding.
    pub identity: Option<Identity>,
    /// Identity displayed after a relaunch.
    pub identity_after_restart: Option<Identity>,
    /// Wallet connection (redacted).
    pub wallet: Option<WalletConnection>,
    /// Whether the app is expected to show a connected wallet. False on
    /// the LND path: the node is Mortsom's and the app is deliberately
    /// walletless, so `wallet` stays `None` on a successful bootstrap.
    #[serde(default = "default_wallet_expected")]
    pub wallet_expected: bool,
}

/// Summaries written before this field existed were all NWC runs.
fn default_wallet_expected() -> bool {
    true
}

/// Runs the per-actor journey: launch, onboard, node and relay, NWC, restart.
/// Each step records evidence and one check; a failed step ends the journey.
pub async fn bootstrap_session(
    config: &Config,
    adapter: &dyn AppAdapter,
    ctx: DeviceContext<'_>,
    env: &TestEnvironment,
    artifacts: &RunArtifacts,
    recorder: &StepRecorder,
    checks: &mut DoctorReport,
) -> ActorSetup {
    let actor = ctx.actor.as_str();
    let wallet = match ctx.actor {
        Actor::Alice => &config.wallets.alice,
        Actor::Bob => &config.wallets.bob,
    };
    let mut outcome = ActorSetup {
        actor: actor.to_owned(),
        identity: None,
        identity_after_restart: None,
        wallet: None,
        wallet_expected: matches!(wallet, WalletConfig::Nwc { .. }),
    };
    let mut step = ActorStep {
        actor,
        ctx: &ctx,
        artifacts,
        recorder,
        checks,
    };
    if step.run("launch", launch_app(&ctx)).await.is_err() {
        return outcome;
    }
    match step.run("onboarding", adapter.onboard(&ctx, env)).await {
        Ok(identity) => outcome.identity = Some(identity),
        Err(()) => return outcome,
    }
    if step
        .run(
            "node-and-relay",
            adapter.configure_node_and_relay(&ctx, env),
        )
        .await
        .is_err()
    {
        return outcome;
    }
    // Only an NWC actor has a wallet to configure into the app. On the LND
    // path the node is Mortsom's, the app stays deliberately walletless,
    // and asking it to show a connected wallet would fail a build that is
    // behaving exactly as intended.
    match wallet {
        WalletConfig::Nwc { uri } => {
            if let Ok(wallet) = step
                .run("nwc", adapter.configure_nwc(&ctx, actor, uri))
                .await
            {
                if !wallet.connected {
                    step.fail("nwc-connected", "app does not show a connected wallet");
                }
                outcome.wallet = Some(wallet);
            }
        }
        WalletConfig::Lnd { .. } => {
            step.skip(
                "nwc",
                "wallet is an LND node held by Mortsom; the app has none by design",
            );
        }
    }
    if let Ok(after) = step.run("restart", relaunch_and_read(adapter, &ctx)).await {
        let same = outcome.identity.as_ref() == Some(&after);
        step.verdict(
            "identity-survives-restart",
            same,
            "same identity after relaunch",
            "identity changed after relaunch",
        );
        outcome.identity_after_restart = Some(after);
    }
    outcome
}

/// Records one named step for an actor: evidence checkpoint plus a check.
struct ActorStep<'a> {
    actor: &'a str,
    ctx: &'a DeviceContext<'a>,
    artifacts: &'a RunArtifacts,
    recorder: &'a StepRecorder,
    checks: &'a mut DoctorReport,
}

impl ActorStep<'_> {
    async fn run<T, E: std::fmt::Display>(
        &mut self,
        name: &str,
        action: impl std::future::Future<Output = Result<T, E>>,
    ) -> Result<T, ()> {
        let result = action.await;
        let suffix = if result.is_ok() { "" } else { "-failed" };
        self.recorder
            .checkpoint(
                self.artifacts,
                self.actor,
                &format!("{name}{suffix}"),
                self.ctx.ui,
            )
            .await;
        match result {
            Ok(value) => {
                self.checks
                    .push(format!("{} {name}", self.actor), CheckStatus::Pass, "ok");
                Ok(value)
            }
            Err(e) => {
                self.checks.push(
                    format!("{} {name}", self.actor),
                    CheckStatus::Fail,
                    e.to_string(),
                );
                Err(())
            }
        }
    }

    fn skip(&mut self, name: &str, detail: &str) {
        self.checks
            .push(format!("{} {name}", self.actor), CheckStatus::Skip, detail);
    }

    fn fail(&mut self, name: &str, detail: &str) {
        self.checks
            .push(format!("{} {name}", self.actor), CheckStatus::Fail, detail);
    }

    fn verdict(&mut self, name: &str, ok: bool, pass: &str, fail: &str) {
        let (status, detail) = if ok {
            (CheckStatus::Pass, pass)
        } else {
            (CheckStatus::Fail, fail)
        };
        self.checks
            .push(format!("{} {name}", self.actor), status, detail);
    }
}

/// Clears app data and brings the app to the foreground.
async fn launch_app(ctx: &DeviceContext<'_>) -> Result<(), String> {
    ctx.ui
        .clear_app()
        .await
        .map_err(|e| format!("clear app: {e}"))?;
    ctx.ui
        .activate_app()
        .await
        .map_err(|e| format!("activate app: {e}"))
}

async fn relaunch_and_read(
    adapter: &dyn AppAdapter,
    ctx: &DeviceContext<'_>,
) -> Result<Identity, String> {
    ctx.ui
        .terminate_app()
        .await
        .map_err(|e| format!("terminate: {e}"))?;
    ctx.ui
        .activate_app()
        .await
        .map_err(|e| format!("activate: {e}"))?;
    adapter
        .displayed_identity(ctx)
        .await
        .map_err(|e| e.to_string())
}
