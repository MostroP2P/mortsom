//! Scenario context: everything a scenario body may touch. It exposes
//! business actions and observations only; the runner owns construction and
//! teardown.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use mortsom_app_contract::{AppAdapter, DeviceContext, TestEnvironment};
use mortsom_common::Actor;
use mortsom_common::{FailureCategory, Lane, Secret};
use mortsom_observers::relay::RelayObserver;
use mortsom_observers::wallet::WalletObserver;
use mortsom_ui_driver::ui::UiSession;
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
    /// Maximum routing fee enforced by this payment edge.
    fn routing_fee_cap_sats(&self) -> u64 {
        0
    }
    /// Issues an invoice for `sats` that expires `expiry_secs` after
    /// issue, for scenarios about invoices the daemon refuses.
    async fn make_invoice_expiring(
        &self,
        _sats: u64,
        _memo: &str,
        _expiry_secs: u64,
    ) -> Result<Secret, String> {
        Err("invoices with a chosen expiry are not supported by this node wallet".into())
    }
    /// Issues a hold invoice for `sats`: a payment to it is accepted and
    /// then held, unresolved, until [`NodeWallet::settle_hold_invoice`].
    /// This is what a payee who pins the daemon's payouts does, so it is
    /// how the daemon's per-destination cap is driven.
    async fn make_hold_invoice(&self, _sats: u64, _memo: &str) -> Result<Secret, String> {
        Err("hold invoices are not supported by this node wallet".into())
    }
    /// Where an invoice this node issued stands: untouched, holding a
    /// locked-in HTLC, settled or cancelled.
    async fn hold_invoice_state(&self, _hash: &str) -> Result<HoldInvoiceState, String> {
        Err("hold invoice state is not supported by this node wallet".into())
    }
    /// Settles a hold invoice this node issued, releasing the HTLC it holds.
    async fn settle_hold_invoice(&self, _hash: &str) -> Result<(), String> {
        Err("hold invoice settlement is not supported by this node wallet".into())
    }
    /// Cancels only a caller-authorized, journal-tracked incoming invoice.
    async fn cancel_invoice(&self, _hash: &str) -> Result<(), String> {
        Err("invoice cancellation is not supported by this node wallet".into())
    }
    /// Counts settled ledger records under this exact hash and direction.
    /// This must not count repeated observations or multipath HTLC attempts.
    async fn settled_record_count(&self, _hash: &str, _incoming: bool) -> Result<usize, String> {
        Err("settlement record enumeration is not supported by this node wallet".into())
    }
    /// Issues an invoice for `sats`, to be typed into the app.
    async fn make_invoice(&self, sats: u64, memo: &str) -> Result<Secret, String>;

    /// Starts paying `invoice`, returning once it is in flight.
    ///
    /// A Mostro hold invoice is held until release, so waiting for
    /// settlement here would stall the trade that is supposed to release
    /// it. Settlement is confirmed by the wallet observer, as always.
    async fn start_payment(&self, invoice: &Secret) -> Result<(), String>;
}

/// Where an invoice a node issued stands, as the node reports it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HoldInvoiceState {
    /// Issued; no payment has reached it.
    Open,
    /// An HTLC is locked in and held: the payer's payment is in flight and
    /// stays so until the invoice is settled or cancelled.
    Accepted,
    /// Settled.
    Settled,
    /// Cancelled, or expired.
    Cancelled,
}

impl HoldInvoiceState {
    /// The stable name used in timelines.
    pub fn as_str(self) -> &'static str {
        match self {
            HoldInvoiceState::Open => "open",
            HoldInvoiceState::Accepted => "accepted",
            HoldInvoiceState::Settled => "settled",
            HoldInvoiceState::Cancelled => "cancelled",
        }
    }
}

/// The daemon's payout limits the stack declares to the run (spec section
/// 5), as far as scenarios need them. `None` means the stack did not
/// declare one; a scenario that needs it says so and fails as environment
/// misconfiguration rather than guessing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct PayoutLimits {
    /// The remaining lifetime a buyer invoice must have when submitted
    /// (`[lightning].invoice_expiration_window`); a shorter one is refused.
    pub payout_invoice_min_expiry_secs: Option<u64>,
    /// How many unresolved payouts the daemon lets one payee hold before
    /// it stops dispatching to that destination
    /// (`[lightning].max_inflight_payouts_per_destination`).
    pub max_inflight_payouts_per_destination: Option<u32>,
}

/// A dispute solver registered with the daemon under test.
///
/// Disputes end only when a solver rules on them, and a solver is a user
/// the daemon's operator registers, not something a client can be. The
/// coordinator registers one per run and hands it to scenarios through
/// this boundary; scenarios never hold keys or talk to the daemon
/// themselves. Every method returns once the daemon has answered.
#[async_trait]
pub trait DisputeSolver: Send + Sync {
    /// Takes the dispute `dispute_id` and returns the id of the order it
    /// is about, as the daemon reported it.
    async fn take_dispute(&self, dispute_id: &str) -> Result<String, String>;
    /// Settles the disputed `order_id` in the buyer's favour: the escrow
    /// is settled and paid out.
    async fn settle(&self, order_id: &str) -> Result<(), String>;
    /// Cancels the disputed `order_id` in the seller's favour: the escrow
    /// is returned.
    async fn cancel(&self, order_id: &str) -> Result<(), String>;
}

/// A hold-invoice payment that is in flight but not yet settled.
///
/// On the node path the daemon holds the HTLC from funding until release,
/// so funding can only confirm the payment is in flight. The reservation
/// stays open (in flight) in the budget until
/// [`crate::actions::settle_hold_payment`] observes the settlement after
/// release, or [`crate::actions::expect_hold_refunded`] observes the refund
/// after a cancellation.
pub struct PendingHold {
    /// Actor whose node pays.
    pub actor: Actor,
    /// Wallet label used by the wallet observer.
    pub wallet_label: String,
    /// Payment hash of the hold invoice.
    pub payment_hash: String,
    /// The budget reservation, committed or released once the fate is known.
    pub reservation: crate::budget::Reservation,
}

/// Everything a scenario can reach.
pub struct Context {
    /// Durable pre-action recovery boundary.
    pub journal: Arc<dyn crate::journal::RecoveryJournal>,
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
    /// The run's dispute solver, when the runner registered one.
    ///
    /// `None` on a run without one: a scenario that needs it is skipped
    /// before it starts (`ScenarioDef::needs_solver`).
    pub solver: Option<Arc<dyn DisputeSolver>>,
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
    /// Hold payments in flight, keyed by order id. See [`PendingHold`].
    pub pending_holds: Mutex<BTreeMap<String, PendingHold>>,
    /// Value-movement clearance; `None` means no value-moving step may run.
    /// Granted by the mainnet guard, or by the runner once every node
    /// wallet proved it is on regtest.
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
    /// The daemon's declared payout limits.
    pub limits: PayoutLimits,
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

    /// The run's dispute solver; an environment error without one.
    pub fn solver(&self) -> Result<&Arc<dyn DisputeSolver>, ScenarioError> {
        self.solver.as_ref().ok_or_else(|| {
            ScenarioError::environment("this run has no dispute solver registered with the daemon")
        })
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

    /// Records a hold payment whose settlement is deferred to release.
    pub fn record_pending_hold(&self, order_id: &str, pending: PendingHold) {
        let mut holds = self.pending_holds.lock().unwrap_or_else(|p| p.into_inner());
        holds.insert(order_id.to_owned(), pending);
    }

    /// Removes and returns the pending hold for `order_id`, if any.
    pub fn take_pending_hold(&self, order_id: &str) -> Option<PendingHold> {
        let mut holds = self.pending_holds.lock().unwrap_or_else(|p| p.into_inner());
        holds.remove(order_id)
    }

    /// Fails unless value-moving steps were authorized.
    pub fn require_mainnet(&self) -> Result<MainnetClearance, ScenarioError> {
        let clearance = self.mainnet.ok_or_else(|| {
            ScenarioError::environment(
                "value-moving step without clearance (mainnet needs run.allow_mainnet and MORTSOM_MAINNET_CONFIRM; regtest needs both wallets on Mortsom-held nodes that report regtest)",
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
