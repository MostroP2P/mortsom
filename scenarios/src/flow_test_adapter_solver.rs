//! The fixture solver: it rules the way the daemon reports a solver's
//! rulings, publishing the dispute status and moving the order.
use super::*;

/// rulings, publishing the dispute status and moving the order.
#[async_trait]
impl mortsom_scenario_kit::context::DisputeSolver for Adapter {
    async fn take_dispute(&self, dispute_id: &str) -> Result<String, String> {
        assert_eq!(dispute_id, DISPUTE);
        self.state
            .lock()
            .unwrap()
            .actions
            .push("solver:take".into());
        self.publish_dispute("in-progress").await;
        let another = self
            .state
            .lock()
            .unwrap()
            .misbehaviour
            .solver_reports_another_order;
        Ok(if another {
            "another-order".to_owned()
        } else {
            ORDER.to_owned()
        })
    }
    async fn settle(&self, order_id: &str) -> Result<(), String> {
        assert_eq!(order_id, ORDER);
        self.state
            .lock()
            .unwrap()
            .actions
            .push("solver:settle".into());
        self.publish_dispute("settled").await;
        {
            let mut state = self.state.lock().unwrap();
            state.dispute_initiator = None;
            let hash = state.buyer_invoice[ORDER].clone();
            state.invoice_mut(&hash).settled = true;
        }
        self.transition(ORDER, OrderState::Completed, "success")
            .await;
        Ok(())
    }
    async fn cancel(&self, order_id: &str) -> Result<(), String> {
        assert_eq!(order_id, ORDER);
        {
            let mut state = self.state.lock().unwrap();
            state.actions.push("solver:cancel".into());
            state.refunded = true;
        }
        self.publish_dispute("seller-refunded").await;
        self.state.lock().unwrap().dispute_initiator = None;
        self.transition(ORDER, OrderState::Cancelled, "canceled")
            .await;
        Ok(())
    }
    async fn settle_slashing(
        &self,
        order_id: &str,
        slash: mortsom_scenario_kit::BondSlash,
    ) -> Result<(), String> {
        self.apply_slash(order_id, slash);
        self.settle(order_id).await
    }
    async fn cancel_slashing(
        &self,
        order_id: &str,
        slash: mortsom_scenario_kit::BondSlash,
    ) -> Result<(), String> {
        self.apply_slash(order_id, slash);
        self.cancel(order_id).await
    }
}
