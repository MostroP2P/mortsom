//! Durable boundary for actions whose outcome must survive coordinator failure.
use mortsom_app_contract::OrderReference;
use mortsom_common::Actor;
use serde::{Deserialize, Serialize};

/// Public recovery facts only; invoice text and private identity material are forbidden.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "kebab-case")]
#[allow(missing_docs)]
pub enum RecoveryEvent {
    /// Written before entering the create journey, including an uncertain submit.
    SubmissionStarted {
        actor: Actor,
        known_orders: Vec<String>,
    },
    /// A newly discovered order shown by its owner's ordinary UI.
    OrderFound { actor: Actor, order: OrderReference },
    /// The operator verified on the daemon and relay that a started
    /// submission never produced an order; recorded through `cleanup`, never
    /// inferred by the harness.
    SubmissionAbandoned { actor: Actor },
    /// Written before submitting an invoice or initiating its payment.
    PaymentExpected {
        order_id: String,
        wallet: String,
        hash: String,
        incoming: bool,
        amount_sats: u64,
    },
    /// Both UI and external payment evidence proved final resolution.
    OrderResolved { order_id: String },
}

/// A successful call means the fact is durable before the next external action.
pub trait RecoveryJournal: Send + Sync {
    /// Whether durable desktop recovery is required.
    fn enabled(&self) -> bool {
        true
    }
    /// Recorded public facts used for exact payment matching.
    fn events(&self) -> Vec<RecoveryEvent> {
        vec![]
    }
    /// Persist one fact before its associated side effect.
    fn record(&self, event: RecoveryEvent) -> Result<(), String>;
}

/// A run that keeps no durable journal: nothing is recorded or resumed.
#[derive(Default)]
pub struct NoJournal;
impl RecoveryJournal for NoJournal {
    fn enabled(&self) -> bool {
        false
    }
    fn record(&self, _: RecoveryEvent) -> Result<(), String> {
        Ok(())
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};
    struct Refuse;
    impl RecoveryJournal for Refuse {
        fn record(&self, _: RecoveryEvent) -> Result<(), String> {
            Err("journal unavailable".into())
        }
    }
    #[tokio::test]
    async fn unavailable_journal_prevents_entering_the_create_journey() {
        let mut ctx = crate::test_support::context(Arc::new(crate::test_support::StateAdapter(
            Mutex::new(mortsom_app_contract::OrderState::Pending),
        )));
        ctx.journal = Arc::new(Refuse);
        let error = crate::actions::create_order(
            &ctx,
            Actor::Alice,
            &mortsom_app_contract::OrderDraft::fixed(
                mortsom_app_contract::OrderKind::Sell,
                100,
                "ARS",
                10,
                "Cash",
            ),
        )
        .await
        .unwrap_err();
        assert!(error.message.contains("journal unavailable"), "{error}");
    }
}
