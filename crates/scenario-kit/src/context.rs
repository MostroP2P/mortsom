//! Scenario context: everything a scenario body may touch. It exposes
//! business actions and observations only; the runner owns construction and
//! teardown.

use std::sync::{Arc, Mutex};
use std::time::Duration;

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
}

impl Context {
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
        self.mainnet.ok_or_else(|| {
            ScenarioError::environment(
                "value-moving step without mainnet clearance (run.allow_mainnet and MORTSOM_MAINNET_CONFIRM)",
            )
        })
    }
}
