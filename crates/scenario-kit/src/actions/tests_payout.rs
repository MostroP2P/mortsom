//! The payout actions against fakes: withdrawing and refusing invoices,
//! proving a superseded payout never settled, and driving hold invoices.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use mortsom_app_contract::{
    AdapterError, AppAdapter, AppOrderState, AppProfile, CancelOutcome, CapabilitySet,
    DeviceContext, Identity, InvoiceSubmission, OrderDraft, OrderReference, OrderState,
    PaymentHandoff, PaymentPath, TestEnvironment, VisibleOrder, WalletConnection,
};
use mortsom_common::{Actor, FailureCategory, Secret};
use mortsom_observers::wallet::{PaymentState, WalletObserver, WalletReader};

use super::*;
use crate::context::{HoldInvoiceState, NodeWallet};
use crate::journal::{RecoveryEvent, RecoveryJournal};
use crate::test_support::{StateAdapter, context};

/// Public BOLT 11 reference invoice (250 000 sats, one-minute lifetime).
const INVOICE: &str = "lnbc2500u1pvjluezsp5zyg3zyg3zyg3zyg3zyg3zyg3zyg3zyg3zyg3zyg3zyg3zyg3zygspp5qqqsyqcyq5rqwzqfqqqsyqcyq5rqwzqfqqqsyqcyq5rqwzqfqypqdq5xysxxatsyp3k7enxv4jsxqzpu9qrsgquk0rl77nj30yxdy8j9vdx85fkpmdla2087ne0xh8nhedh8w27kyke0lp53ut353s06fv3qfegext0eh0ymjpf39tuven09sam30g4vgpfna3rh";

/// Everything the fake node was asked to do, and what it answers.
struct Node {
    calls: Mutex<Vec<String>>,
    hold: Mutex<HoldInvoiceState>,
    settled_records: usize,
    payment: PaymentState,
}

impl Default for Node {
    fn default() -> Self {
        Self {
            calls: Mutex::new(Vec::new()),
            hold: Mutex::new(HoldInvoiceState::Open),
            settled_records: 0,
            payment: PaymentState::Failed,
        }
    }
}

#[async_trait]
impl NodeWallet for Node {
    async fn make_invoice(&self, _: u64, _: &str) -> Result<Secret, String> {
        Ok(Secret::new(INVOICE))
    }
    async fn make_invoice_expiring(&self, _: u64, _: &str, secs: u64) -> Result<Secret, String> {
        self.calls
            .lock()
            .unwrap()
            .push(format!("issue expiring {secs}"));
        Ok(Secret::new(INVOICE))
    }
    async fn cancel_invoice(&self, hash: &str) -> Result<(), String> {
        self.calls.lock().unwrap().push(format!("cancel {hash}"));
        Ok(())
    }
    async fn settled_record_count(&self, _: &str, incoming: bool) -> Result<usize, String> {
        assert!(incoming);
        Ok(self.settled_records)
    }
    async fn hold_invoice_state(&self, _: &str) -> Result<HoldInvoiceState, String> {
        Ok(*self.hold.lock().unwrap())
    }
    async fn settle_hold_invoice(&self, hash: &str) -> Result<(), String> {
        self.calls.lock().unwrap().push(format!("settle {hash}"));
        *self.hold.lock().unwrap() = HoldInvoiceState::Settled;
        Ok(())
    }
    async fn start_payment(&self, _: &Secret) -> Result<(), String> {
        Ok(())
    }
}

#[async_trait]
impl WalletReader for Node {
    async fn lookup(&self, _: &str) -> Result<Option<(u64, u64, PaymentState)>, String> {
        Ok(Some((250_000, 0, self.payment)))
    }
}

/// Records every journal fact.
#[derive(Default)]
struct Recorder(Mutex<Vec<RecoveryEvent>>);
impl RecoveryJournal for Recorder {
    fn events(&self) -> Vec<RecoveryEvent> {
        self.0.lock().unwrap().clone()
    }
    fn record(&self, event: RecoveryEvent) -> Result<(), String> {
        self.0.lock().unwrap().push(event);
        Ok(())
    }
}

/// A state adapter whose manual invoice form answers with a fixed verdict.
struct Verdict(StateAdapter, InvoiceSubmission);
#[async_trait]
impl AppAdapter for Verdict {
    fn profile(&self) -> AppProfile {
        self.0.profile()
    }
    fn capabilities(&self) -> CapabilitySet {
        self.0.capabilities()
    }
    async fn onboard(
        &self,
        d: &DeviceContext<'_>,
        e: &TestEnvironment,
    ) -> Result<Identity, AdapterError> {
        self.0.onboard(d, e).await
    }
    async fn displayed_identity(&self, d: &DeviceContext<'_>) -> Result<Identity, AdapterError> {
        self.0.displayed_identity(d).await
    }
    async fn configure_node_and_relay(
        &self,
        d: &DeviceContext<'_>,
        e: &TestEnvironment,
    ) -> Result<(), AdapterError> {
        self.0.configure_node_and_relay(d, e).await
    }
    async fn configure_nwc(
        &self,
        d: &DeviceContext<'_>,
        l: &str,
        u: &Secret,
    ) -> Result<WalletConnection, AdapterError> {
        self.0.configure_nwc(d, l, u).await
    }
    async fn create_order(
        &self,
        d: &DeviceContext<'_>,
        o: &OrderDraft,
    ) -> Result<VisibleOrder, AdapterError> {
        self.0.create_order(d, o).await
    }
    async fn find_and_take_order(
        &self,
        d: &DeviceContext<'_>,
        o: &OrderReference,
    ) -> Result<(), AdapterError> {
        self.0.find_and_take_order(d, o).await
    }
    async fn pay_hold_invoice(
        &self,
        d: &DeviceContext<'_>,
        o: &OrderReference,
        p: PaymentPath,
    ) -> Result<PaymentHandoff, AdapterError> {
        self.0.pay_hold_invoice(d, o, p).await
    }
    async fn buyer_invoice_amount(
        &self,
        _: &DeviceContext<'_>,
        _: &OrderReference,
    ) -> Result<u64, AdapterError> {
        Ok(250_000)
    }
    async fn provide_buyer_invoice(
        &self,
        d: &DeviceContext<'_>,
        o: &OrderReference,
        i: Option<&Secret>,
    ) -> Result<Option<Secret>, AdapterError> {
        self.0.provide_buyer_invoice(d, o, i).await
    }
    async fn submit_buyer_invoice(
        &self,
        _: &DeviceContext<'_>,
        _: &OrderReference,
        _: &Secret,
    ) -> Result<InvoiceSubmission, AdapterError> {
        Ok(self.1.clone())
    }
    async fn confirm_fiat_sent(
        &self,
        d: &DeviceContext<'_>,
        o: &OrderReference,
    ) -> Result<(), AdapterError> {
        self.0.confirm_fiat_sent(d, o).await
    }
    async fn release(&self, d: &DeviceContext<'_>, o: &OrderReference) -> Result<(), AdapterError> {
        self.0.release(d, o).await
    }
    async fn cancel_if_open(
        &self,
        d: &DeviceContext<'_>,
        o: &OrderReference,
    ) -> Result<CancelOutcome, AdapterError> {
        self.0.cancel_if_open(d, o).await
    }
    async fn visible_actions(
        &self,
        d: &DeviceContext<'_>,
        o: &OrderReference,
    ) -> Result<Vec<String>, AdapterError> {
        self.0.visible_actions(d, o).await
    }
    async fn cancel_payment(
        &self,
        d: &DeviceContext<'_>,
        o: &OrderReference,
    ) -> Result<(), AdapterError> {
        self.0.cancel_payment(d, o).await
    }
    async fn observe_order(
        &self,
        d: &DeviceContext<'_>,
        o: &OrderReference,
    ) -> Result<AppOrderState, AdapterError> {
        self.0.observe_order(d, o).await
    }
}

/// A context with Bob's node and wallet reader both answered by `node`.
fn with_node(adapter: Arc<dyn AppAdapter>, node: Arc<Node>) -> (Context, Arc<Recorder>) {
    let mut ctx = context(adapter);
    let journal = Arc::new(Recorder::default());
    ctx.journal = journal.clone();
    ctx.node_wallets
        .insert("bob".into(), node.clone() as Arc<dyn NodeWallet>);
    ctx.wallets = Arc::new(WalletObserver::new(vec![(
        "bob".into(),
        node as Arc<dyn WalletReader>,
    )]));
    ctx.amount_sats = 250_000;
    ctx.settle_timeout = Duration::from_millis(50);
    (ctx, journal)
}

fn hash() -> String {
    payment_hash_of(&Secret::new(INVOICE)).unwrap()
}

#[tokio::test]
async fn a_refused_invoice_is_superseded_in_the_journal_and_withdrawn_on_the_node() {
    let node = Arc::new(Node::default());
    let adapter = Arc::new(Verdict(
        StateAdapter(Mutex::new(OrderState::Taken)),
        InvoiceSubmission::Rejected {
            reason: "The node rejected this invoice.".into(),
        },
    ));
    let (ctx, journal) = with_node(adapter, node.clone());
    let order = OrderReference::new("o1");

    let refused = submit_rejected_buyer_invoice(
        &ctx,
        Actor::Bob,
        &order,
        BuyerInvoice::Expiring { secs: 60 },
    )
    .await
    .unwrap();

    assert_eq!(refused.hash, hash());
    assert_eq!(refused.reason, "The node rejected this invoice.");
    assert_eq!(
        node.calls.lock().unwrap().as_slice(),
        ["issue expiring 60".to_owned(), format!("cancel {}", hash())]
    );
    // Prepared first, superseded after: the replacement is a new intent.
    let events = journal.events();
    assert!(matches!(
        &events[0],
        RecoveryEvent::PaymentExpected { order_id, wallet, hash: h, incoming: true, amount_sats: 250_000 }
            if order_id == "o1" && wallet == "bob" && *h == hash()
    ));
    assert!(matches!(
        &events[1],
        RecoveryEvent::PaymentSuperseded { order_id, wallet, hash: h }
            if order_id == "o1" && wallet == "bob" && *h == hash()
    ));
}

/// The daemon taking the invoice it should have refused is the product
/// failure this action exists to catch; nothing is withdrawn then.
#[tokio::test]
async fn an_accepted_invoice_that_should_have_been_refused_is_a_product_failure() {
    let node = Arc::new(Node::default());
    let adapter = Arc::new(Verdict(
        StateAdapter(Mutex::new(OrderState::Taken)),
        InvoiceSubmission::Accepted,
    ));
    let (ctx, journal) = with_node(adapter, node.clone());

    let error = submit_rejected_buyer_invoice(
        &ctx,
        Actor::Bob,
        &OrderReference::new("o1"),
        BuyerInvoice::Expiring { secs: 60 },
    )
    .await
    .unwrap_err();

    assert_eq!(error.category, FailureCategory::Product);
    assert!(error.message.contains("should have refused"), "{error}");
    assert!(
        !node
            .calls
            .lock()
            .unwrap()
            .iter()
            .any(|call| call.starts_with("cancel"))
    );
    assert_eq!(journal.events().len(), 1, "prepared, never superseded");
}

#[tokio::test]
async fn a_lifetime_the_node_did_not_encode_is_caught_before_submission() {
    let node = Arc::new(Node::default());
    let adapter = Arc::new(Verdict(
        StateAdapter(Mutex::new(OrderState::Taken)),
        InvoiceSubmission::Accepted,
    ));
    let (ctx, _) = with_node(adapter, node);

    let error = submit_rejected_buyer_invoice(
        &ctx,
        Actor::Bob,
        &OrderReference::new("o1"),
        BuyerInvoice::Expiring { secs: 120 },
    )
    .await
    .unwrap_err();

    assert_eq!(error.category, FailureCategory::Wallet);
    assert!(error.message.contains("60s lifetime"), "{error}");
}

#[tokio::test]
async fn withdrawing_an_invoice_needs_a_node_mortsom_holds() {
    let ctx = context(Arc::new(StateAdapter(Mutex::new(OrderState::Active))));
    let error = supersede_buyer_invoice(&ctx, Actor::Bob, &OrderReference::new("o1"), &hash())
        .await
        .unwrap_err();
    assert_eq!(error.category, FailureCategory::EnvironmentMisconfiguration);
}

#[tokio::test]
async fn a_superseded_payout_is_safe_only_when_failed_with_no_settled_record() {
    let order = OrderReference::new("o1");
    for (payment, records, safe) in [
        (PaymentState::Failed, 0, true),
        (PaymentState::Settled, 1, false),
        (PaymentState::Failed, 1, false),
    ] {
        let node = Arc::new(Node {
            payment,
            settled_records: records,
            ..Node::default()
        });
        let (ctx, _) = with_node(
            Arc::new(StateAdapter(Mutex::new(OrderState::Completed))),
            node,
        );
        let result = expect_payout_never_settled(&ctx, Actor::Bob, &order, &hash()).await;
        assert_eq!(result.is_ok(), safe, "{payment:?}/{records}: {result:?}");
        if let Err(error) = result {
            assert_eq!(error.category, FailureCategory::Product);
        }
    }
}

#[tokio::test]
async fn a_hold_invoice_is_waited_on_and_watched_and_settled() {
    let node = Arc::new(Node::default());
    let (mut ctx, _) = with_node(
        Arc::new(StateAdapter(Mutex::new(OrderState::PayoutPending))),
        node.clone(),
    );
    ctx.state_timeout = Duration::from_millis(200);
    let order = OrderReference::new("o1");
    let h = hash();

    // Untouched for the whole window.
    expect_hold_invoice_stays(
        &ctx,
        Actor::Bob,
        &order,
        &h,
        HoldInvoiceState::Open,
        Duration::from_millis(30),
    )
    .await
    .unwrap();

    // Accepted a moment later: the watch reports it, the wait sees it.
    let flip = node.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(20)).await;
        *flip.hold.lock().unwrap() = HoldInvoiceState::Accepted;
    });
    let error = expect_hold_invoice_stays(
        &ctx,
        Actor::Bob,
        &order,
        &h,
        HoldInvoiceState::Open,
        Duration::from_millis(500),
    )
    .await
    .unwrap_err();
    assert_eq!(error.category, FailureCategory::Product);
    assert!(error.message.contains("had to stay open"), "{error}");
    wait_hold_invoice(
        &ctx,
        Actor::Bob,
        &order,
        &h,
        HoldInvoiceState::Accepted,
        Duration::from_millis(500),
    )
    .await
    .unwrap();

    settle_buyer_hold_invoice(&ctx, Actor::Bob, &order, &h)
        .await
        .unwrap();
    assert!(node.calls.lock().unwrap().contains(&format!("settle {h}")));

    // A settled invoice never becomes accepted again: the deadline says so.
    let error = wait_hold_invoice(
        &ctx,
        Actor::Bob,
        &order,
        &h,
        HoldInvoiceState::Accepted,
        Duration::from_millis(20),
    )
    .await
    .unwrap_err();
    assert_eq!(error.category, FailureCategory::Product);

    // A cancelled one fails the wait at once, as a wallet fact.
    *node.hold.lock().unwrap() = HoldInvoiceState::Cancelled;
    let error = wait_hold_invoice(
        &ctx,
        Actor::Bob,
        &order,
        &h,
        HoldInvoiceState::Accepted,
        Duration::from_millis(500),
    )
    .await
    .unwrap_err();
    assert_eq!(error.category, FailureCategory::Wallet);
}

/// After a failure, an accepted hold is settled and an untouched one is
/// withdrawn, so nothing stays pinned on the buyer's node.
#[tokio::test]
async fn held_payouts_are_released_after_a_failure() {
    let accepted = Arc::new(Node::default());
    *accepted.hold.lock().unwrap() = HoldInvoiceState::Accepted;
    let (ctx, journal) = with_node(
        Arc::new(StateAdapter(Mutex::new(OrderState::PayoutPending))),
        accepted.clone(),
    );
    let held = [(OrderReference::new("o1"), hash())];
    assert!(
        release_held_payouts(&ctx, Actor::Bob, &held)
            .await
            .is_empty()
    );
    assert_eq!(
        accepted.calls.lock().unwrap().as_slice(),
        [format!("settle {}", hash())]
    );
    assert!(journal.events().is_empty());

    let open = Arc::new(Node::default());
    let (ctx, journal) = with_node(
        Arc::new(StateAdapter(Mutex::new(OrderState::PayoutPending))),
        open.clone(),
    );
    assert!(
        release_held_payouts(&ctx, Actor::Bob, &held)
            .await
            .is_empty()
    );
    assert_eq!(
        open.calls.lock().unwrap().as_slice(),
        [format!("cancel {}", hash())]
    );
    assert_eq!(journal.events().len(), 1, "withdrawn in the journal first");

    let ctx = context(Arc::new(StateAdapter(Mutex::new(OrderState::Active))));
    assert_eq!(release_held_payouts(&ctx, Actor::Bob, &held).await.len(), 1);
}
