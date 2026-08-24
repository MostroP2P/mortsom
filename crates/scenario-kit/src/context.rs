//! Scenario context: everything a scenario body may touch. It exposes
//! business actions and observations only; the runner owns construction and
//! teardown.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use mortsom_android_driver::avd::Actor;
use mortsom_android_driver::ui::UiSession;
use mortsom_app_contract::{AppAdapter, DeviceContext, TestEnvironment};
use mortsom_common::{FailureCategory, Lane, Secret};
use mortsom_observers::relay::RelayObserver;
use mortsom_observers::wallet::WalletObserver;
use thiserror::Error;

use crate::budget::Budget;
use crate::cleanup::CleanupRegistry;
use crate::evidence::EvidenceSink;
use crate::recovery::RelayControl;
use crate::timeline::Timeline;

/// A scenario failure with its explicit category.
#[derive(Debug, Error, Clone, PartialEq, Eq)]
#[error("{category}: {message}")]
pub struct ScenarioError {
    /// Why.
    pub category: FailureCategory,
    /// Redacted detail.
    pub message: String,
}

impl ScenarioError {
    /// Builds an error.
    pub fn new(category: FailureCategory, message: impl Into<String>) -> Self {
        Self {
            category,
            message: message.into(),
        }
    }

    /// The app misbehaved.
    pub fn product(message: impl Into<String>) -> Self {
        Self::new(FailureCategory::Product, message)
    }

    /// Relay evidence missing or wrong.
    pub fn relay(message: impl Into<String>) -> Self {
        Self::new(FailureCategory::Relay, message)
    }

    /// Wallet evidence missing or wrong.
    pub fn wallet(message: impl Into<String>) -> Self {
        Self::new(FailureCategory::Wallet, message)
    }

    /// Configuration or safety refusal.
    pub fn environment(message: impl Into<String>) -> Self {
        Self::new(FailureCategory::EnvironmentMisconfiguration, message)
    }
}

impl From<mortsom_app_contract::AdapterError> for ScenarioError {
    fn from(e: mortsom_app_contract::AdapterError) -> Self {
        Self::new(e.category, e.message)
    }
}

/// Proof that the runner authorized value-moving steps for this run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MainnetClearance {
    /// Lane the clearance was granted for.
    pub lane: Lane,
}

/// One actor's device and wallet within a scenario.
pub struct ActorSession {
    /// Actor.
    pub actor: Actor,
    /// UI session.
    pub ui: Arc<dyn UiSession>,
    /// Wallet label used by the wallet observer.
    pub wallet_label: String,
}

impl ActorSession {
    /// The adapter-facing device view.
    pub fn device(&self, ui_timeout: Duration) -> DeviceContext<'_> {
        DeviceContext {
            actor: self.actor,
            ui: self.ui.as_ref(),
            ui_timeout,
        }
    }
}

/// A Lightning node Mortsom holds and pays with directly.
///
/// This is what a local Polar network gives a run that NWC cannot: the
/// wallet is ours, so a whole trade settles in a fraction of a second on
/// regtest with worthless sats, instead of costing mainnet sats and a
/// wallet service's latency.
#[async_trait]
pub trait NodeWallet: Send + Sync {
    /// Issues an invoice for `sats`, to be typed into the app.
    async fn make_invoice(&self, sats: u64, memo: &str) -> Result<Secret, String>;

    /// Starts paying `invoice`, returning once it is in flight.
    ///
    /// A Mostro hold invoice is held until release, so waiting for
    /// settlement here would stall the trade that is supposed to release
    /// it. Settlement is confirmed by the wallet observer, as always.
    async fn start_payment(&self, invoice: &Secret) -> Result<(), String>;
}

/// Everything a scenario can reach.
pub struct Context {
    /// Application adapter.
    pub adapter: Arc<dyn AppAdapter>,
    /// Alice and Bob.
    pub actors: Vec<ActorSession>,
    /// The environment both apps point at.
    pub env: TestEnvironment,
    /// Relay observer (order ids are added as orders appear).
    pub relay: Arc<RelayObserver>,
    /// Wallet observer.
    pub wallets: Arc<WalletObserver>,
    /// Nodes Mortsom holds, keyed by actor label.
    ///
    /// Empty on an NWC run: there the app has its own wallet and pays for
    /// itself. An entry here means the app has no wallet connected, so
    /// Mortsom issues the buyer invoice and pays the hold invoice through
    /// the app's manual controls instead.
    pub node_wallets: BTreeMap<String, Arc<dyn NodeWallet>>,
    /// Relay fault injection (restart, replay).
    pub relay_control: Arc<dyn RelayControl>,
    /// Cap accounting.
    pub budget: Arc<Mutex<Budget>>,
    /// Cleanup registry.
    pub cleanup: Arc<CleanupRegistry>,
    /// Ordered timeline.
    pub timeline: Arc<Timeline>,
    /// Evidence sink.
    pub evidence: Arc<dyn EvidenceSink>,
    /// Mainnet clearance; `None` means no value-moving step may run.
    pub mainnet: Option<MainnetClearance>,
    /// Sats per order.
    pub amount_sats: u64,
    /// Fiat code for drafts.
    pub fiat_code: String,
    /// Fiat amount for drafts.
    pub fiat_amount: u64,
    /// Payment method for drafts.
    pub payment_method: String,
    /// Deadline for one UI wait.
    pub ui_timeout: Duration,
    /// Deadline for a state to be reached across devices and relay.
    pub state_timeout: Duration,
    /// Deadline for a payment to settle.
    pub settle_timeout: Duration,
    /// Deadline for the daemon to expire an unpaid taken order.
    pub expiration_timeout: Duration,
    /// Interruption timing for recovery scenarios.
    pub recovery: RecoveryTiming,
}

/// How long recovery scenarios interrupt and then watch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RecoveryTiming {
    /// Window after an interruption in which no repeated action may appear.
    pub quiet_window: Duration,
    /// Seconds an app is kept in the background.
    pub background_secs: u64,
    /// Length of an injected connectivity outage.
    pub outage: Duration,
}

impl Context {
    /// The node backing `actor`, when Mortsom holds one.
    ///
    /// `None` means the app has its own wallet and pays for itself.
    pub fn node_wallet(&self, actor: Actor) -> Option<&Arc<dyn NodeWallet>> {
        self.node_wallets.get(actor.as_str())
    }

    /// The session of `actor`.
    pub fn actor(&self, actor: Actor) -> Result<&ActorSession, ScenarioError> {
        self.actors
            .iter()
            .find(|a| a.actor == actor)
            .ok_or_else(|| {
                ScenarioError::environment(format!("actor {actor} is not part of this run"))
            })
    }

    /// Registers a secret learned mid-run (invoices) with the evidence sink.
    pub fn protect(&self, secret: &Secret) {
        self.evidence.protect(secret);
    }

    /// Fails unless value-moving steps were authorized.
    pub fn require_mainnet(&self) -> Result<MainnetClearance, ScenarioError> {
        let clearance = self.mainnet.ok_or_else(|| {
            ScenarioError::environment(
                "value-moving step without mainnet clearance (run.allow_mainnet and MORTSOM_MAINNET_CONFIRM)",
            )
        })?;
        // A clearance authorizes the lane it was granted for; carrying one
        // over into another lane would spend under the wrong approval.
        if clearance.lane != self.env.lane {
            return Err(ScenarioError::environment(format!(
                "mainnet clearance was granted for the {} lane but the run is on {}",
                clearance.lane, self.env.lane
            )));
        }
        Ok(clearance)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    /// A clearance approves one lane. Reusing it on another lane would
    /// spend real sats under an approval that was never given for it.
    #[test]
    fn a_clearance_from_another_lane_does_not_authorize_this_run() {
        let mut ctx =
            crate::test_support::context(std::sync::Arc::new(crate::test_support::StateAdapter(
                std::sync::Mutex::new(mortsom_app_contract::OrderState::Pending),
            )));
        ctx.mainnet = Some(MainnetClearance { lane: Lane::Canary });

        let err = ctx.require_mainnet().unwrap_err();
        assert!(err.message.contains("lane"), "{err}");

        ctx.env.lane = Lane::Canary;
        assert!(ctx.require_mainnet().is_ok());
    }
}
