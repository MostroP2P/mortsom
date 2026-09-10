#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::{Arc, Mutex};

use super::*;
use crate::test_support::{StateAdapter, context};

#[tokio::test]
async fn expect_state_holds_watches_the_whole_window() {
    let adapter = Arc::new(StateAdapter(Mutex::new(OrderState::Active)));
    let ctx = context(adapter.clone());
    let order = OrderReference::new("o1");
    expect_state_holds(
        &ctx,
        Actor::Bob,
        &order,
        OrderState::Active,
        Duration::from_millis(30),
    )
    .await
    .unwrap();

    let flipper = adapter.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(20)).await;
        *flipper.0.lock().unwrap() = OrderState::Cancelled;
    });
    let err = expect_state_holds(
        &ctx,
        Actor::Bob,
        &order,
        OrderState::Active,
        Duration::from_millis(500),
    )
    .await
    .unwrap_err();
    assert!(err.message.contains("left active (`cancelled`)"), "{err}");
}

#[tokio::test]
async fn wait_app_state_fails_fast_on_a_foreign_terminal_state() {
    let adapter = Arc::new(StateAdapter(Mutex::new(OrderState::Cancelled)));
    let ctx = context(adapter);
    let order = OrderReference::new("o1");
    let started = std::time::Instant::now();

    let err = wait_app_state(&ctx, Actor::Alice, &order, OrderState::Active)
        .await
        .unwrap_err();

    assert!(err.message.contains("terminal `cancelled`"));
    assert!(
        started.elapsed() < Duration::from_millis(250),
        "must not wait out the deadline"
    );
}

#[tokio::test]
async fn wait_app_state_keeps_waiting_through_earlier_states_and_times_out() {
    let adapter = Arc::new(StateAdapter(Mutex::new(OrderState::Pending)));
    let ctx = context(adapter.clone());
    let order = OrderReference::new("o1");
    let err = wait_app_state(&ctx, Actor::Alice, &order, OrderState::Taken)
        .await
        .unwrap_err();
    assert!(err.message.contains("did not reach taken"));
    assert!(
        err.message.contains("last showed `pending`"),
        "the deadline must name what the app actually showed: {err}"
    );

    *adapter.0.lock().unwrap() = OrderState::Taken;
    let raw = wait_app_state(&ctx, Actor::Alice, &order, OrderState::Taken)
        .await
        .unwrap();
    assert_eq!(raw, "taken");
}

/// A wait that ends without its state has to distinguish the app showing
/// something else from the app never being read: the first is the
/// product, the second is the harness, and a message that says neither
/// sends the reader to the wrong half of the system.
#[test]
fn a_state_deadline_says_what_the_last_observation_was() {
    let order = OrderReference::new("o1");
    let timeout = Duration::from_secs(180);

    let mut seen = LastSeen::default();
    seen.read("waiting-payment".into());
    let showed = state_timeout_message(&order, "pending", Actor::Alice, timeout, &seen);
    assert!(showed.contains("last showed `waiting-payment`"), "{showed}");

    let mut seen = LastSeen::default();
    seen.failed("trades.item.o1 not found".into());
    let blind = state_timeout_message(&order, "pending", Actor::Alice, timeout, &seen);
    assert!(
        blind.contains("was never read: trades.item.o1 not found"),
        "{blind}"
    );
    assert!(
        !blind.contains("last showed"),
        "an unread app is not a state the app showed: {blind}"
    );

    let never = state_timeout_message(
        &order,
        "pending",
        Actor::Alice,
        timeout,
        &LastSeen::default(),
    );
    assert!(never.contains("the app was never read"), "{never}");
}

/// A read that fails after a successful one does not unsee it. Calling
/// such a wait blind sends the reader after a navigation bug that never
/// happened -- the app was read, repeatedly, and showed the wrong state;
/// only the last poll went unanswered. The deadline owes both halves.
#[test]
fn a_failed_read_does_not_erase_the_status_the_app_did_show() {
    let order = OrderReference::new("o1");
    let timeout = Duration::from_secs(180);

    let mut seen = LastSeen::default();
    seen.read("waiting-payment".into());
    seen.failed("webdriver transport error".into());

    let both = state_timeout_message(&order, "pending", Actor::Alice, timeout, &seen);
    assert!(
        both.contains("last showed `waiting-payment`"),
        "a status read before the failure is still what the app showed: {both}"
    );
    assert!(
        both.contains("could not be read: webdriver transport error"),
        "the read that failed last is why the wait has nothing newer: {both}"
    );
    assert!(
        !both.contains("was never read:"),
        "an app read many times was not never read: {both}"
    );
}

/// The other direction: a blip the next poll answered is not what the
/// deadline is about, so a successful read clears the failure before it.
#[test]
fn a_read_that_succeeds_clears_the_failure_before_it() {
    let order = OrderReference::new("o1");
    let mut seen = LastSeen::default();
    seen.failed("webdriver transport error".into());
    seen.read("waiting-payment".into());

    let msg = state_timeout_message(
        &order,
        "pending",
        Actor::Alice,
        Duration::from_secs(180),
        &seen,
    );
    assert_eq!(
        msg,
        format!(
            "order o1 did not reach pending on alice within {:?}; the app last showed `waiting-payment`",
            Duration::from_secs(180)
        ),
        "a failure the next poll answered has no place in the verdict: {msg}"
    );
}

#[tokio::test]
async fn value_moving_actions_require_clearance_before_touching_the_wallet() {
    let ctx = context(Arc::new(StateAdapter(Mutex::new(OrderState::Taken))));
    let err = pay_hold_invoice(&ctx, Actor::Alice, &OrderReference::new("o1"))
        .await
        .unwrap_err();
    assert_eq!(
        err.category,
        mortsom_common::FailureCategory::EnvironmentMisconfiguration
    );
    assert!(
        ctx.budget.lock().unwrap().ledger().is_empty(),
        "nothing reserved without clearance"
    );
}
/// Without a pending hold (the NWC path) release and cancel have no
/// deferred settlement to resolve; both must be clean no-ops.
#[tokio::test]
async fn hold_resolution_is_a_no_op_without_a_pending_hold() {
    let ctx = context(Arc::new(StateAdapter(Mutex::new(OrderState::Completed))));
    let order = OrderReference::new("o1");

    assert_eq!(settle_hold_payment(&ctx, &order).await.unwrap(), None);
    expect_hold_refunded(&ctx, &order).await.unwrap();
    assert!(ctx.budget.lock().unwrap().ledger().is_empty());
}

/// After release the pending hold must settle and its reservation is
/// committed against the budget, fee included; after a cancellation
/// the payment must fail and the reservation is freed.
#[tokio::test]
async fn a_pending_hold_settles_after_release_and_refunds_after_cancel() {
    use crate::budget::EntryStatus;
    use crate::context::PendingHold;

    let mut ctx = context(Arc::new(StateAdapter(Mutex::new(OrderState::Completed))));
    ctx.wallets = Arc::new(mortsom_observers::wallet::WalletObserver::new(vec![(
        "alice".into(),
        Arc::new(SettledWallet) as Arc<dyn mortsom_observers::wallet::WalletReader>,
    )]));
    let order = OrderReference::new("o1");
    let reservation = ctx.budget.lock().unwrap().reserve("o1", 5).unwrap();
    ctx.record_pending_hold(
        "o1",
        PendingHold {
            actor: Actor::Alice,
            wallet_label: "alice".into(),
            payment_hash: "aa".into(),
            reservation,
        },
    );

    let obs = settle_hold_payment(&ctx, &order).await.unwrap().unwrap();

    assert_eq!(obs.state, PaymentState::Settled);
    {
        let budget = ctx.budget.lock().unwrap();
        assert_eq!(budget.ledger()[0].status, EntryStatus::Settled);
        assert_eq!(budget.ledger()[0].fee_sats, 1);
    }

    // A second call finds nothing: the hold was consumed.
    assert_eq!(settle_hold_payment(&ctx, &order).await.unwrap(), None);

    // Cancellation path: the payment fails and the budget is freed.
    ctx.wallets = Arc::new(mortsom_observers::wallet::WalletObserver::new(vec![(
        "alice".into(),
        Arc::new(FailedWallet) as Arc<dyn mortsom_observers::wallet::WalletReader>,
    )]));
    // 6 of the 10-sat run cap are already settled (5 + fee 1).
    let order2 = OrderReference::new("o2");
    let reservation = ctx.budget.lock().unwrap().reserve("o2", 3).unwrap();
    ctx.record_pending_hold(
        "o2",
        PendingHold {
            actor: Actor::Alice,
            wallet_label: "alice".into(),
            payment_hash: "bb".into(),
            reservation,
        },
    );

    expect_hold_refunded(&ctx, &order2).await.unwrap();

    let budget = ctx.budget.lock().unwrap();
    assert_eq!(budget.ledger()[1].status, EntryStatus::Released);
}

struct SettledWallet;
#[async_trait::async_trait]
impl mortsom_observers::wallet::WalletReader for SettledWallet {
    async fn lookup(&self, _: &str) -> Result<Option<(u64, u64, PaymentState)>, String> {
        Ok(Some((5, 1, PaymentState::Settled)))
    }
}

struct FailedWallet;
#[async_trait::async_trait]
impl mortsom_observers::wallet::WalletReader for FailedWallet {
    async fn lookup(&self, _: &str) -> Result<Option<(u64, u64, PaymentState)>, String> {
        Ok(Some((5, 0, PaymentState::Failed)))
    }
}

/// The sats left the wallet even when the ledger refuses the amount, so
/// the reservation may not stay `Reserved` and drop out of the spend.
#[test]
fn a_refused_settlement_is_still_accounted_for() {
    use crate::budget::{Budget, EntryStatus, SpendCaps};
    use mortsom_observers::wallet::{PaymentObservation, PaymentState};

    let mut budget = Budget::new(SpendCaps::new(100, 100).unwrap());
    let reservation = budget.reserve("o1", 10).unwrap();
    let observation = PaymentObservation {
        wallet: "alice".into(),
        order_id: "o1".into(),
        payment_hash: "aa".into(),
        amount_sats: 50,
        fee_sats: 1,
        state: PaymentState::Settled,
        observed_at: 1,
    };

    let refused = commit_settlement(&mut budget, &reservation, &observation, "aa").unwrap_err();

    assert!(matches!(
        refused,
        crate::budget::BudgetError::Overspend { .. }
    ));
    let entry = &budget.ledger()[0];
    assert_eq!(entry.status, EntryStatus::InFlight, "{entry:?}");
    assert_eq!(entry.payment_hash.as_deref(), Some("aa"));
    assert!(budget.totals().reserved_sats > 0, "still counted as spend");
}

/// An app whose pay form is still open from the take: the form keeps
/// saying `waiting-payment`, while the trade detail already shows what the
/// daemon did. A wait for a state the form can never become must read the
/// detail; a wait for one the form flows into on its own keeps reading the
/// form.
mod form_left_open {
    use std::sync::{Arc, Mutex};

    use async_trait::async_trait;
    use mortsom_app_contract::{
        AdapterError, AppOrderState, DeviceContext, OrderReference, OrderState,
    };

    use super::super::*;
    use crate::test_support::{StateAdapter, context};

    struct FormOpen {
        detail: StateAdapter,
        detail_reads: Mutex<u32>,
    }

    #[async_trait]
    impl mortsom_app_contract::AppAdapter for FormOpen {
        fn profile(&self) -> mortsom_app_contract::AppProfile {
            self.detail.profile()
        }
        fn capabilities(&self) -> mortsom_app_contract::CapabilitySet {
            self.detail.capabilities()
        }
        async fn build(
            &self,
            c: &mortsom_app_contract::BuildContext,
            r: &dyn mortsom_android_driver::process::CommandRunner,
        ) -> Result<mortsom_app_contract::Apk, AdapterError> {
            self.detail.build(c, r).await
        }
        async fn onboard(
            &self,
            d: &DeviceContext<'_>,
            e: &mortsom_app_contract::TestEnvironment,
        ) -> Result<mortsom_app_contract::Identity, AdapterError> {
            self.detail.onboard(d, e).await
        }
        async fn displayed_identity(
            &self,
            d: &DeviceContext<'_>,
        ) -> Result<mortsom_app_contract::Identity, AdapterError> {
            self.detail.displayed_identity(d).await
        }
        async fn configure_node_and_relay(
            &self,
            d: &DeviceContext<'_>,
            e: &mortsom_app_contract::TestEnvironment,
        ) -> Result<(), AdapterError> {
            self.detail.configure_node_and_relay(d, e).await
        }
        async fn configure_nwc(
            &self,
            d: &DeviceContext<'_>,
            l: &str,
            u: &mortsom_common::Secret,
        ) -> Result<mortsom_app_contract::WalletConnection, AdapterError> {
            self.detail.configure_nwc(d, l, u).await
        }
        async fn create_order(
            &self,
            d: &DeviceContext<'_>,
            o: &mortsom_app_contract::OrderDraft,
        ) -> Result<mortsom_app_contract::VisibleOrder, AdapterError> {
            self.detail.create_order(d, o).await
        }
        async fn find_and_take_order(
            &self,
            d: &DeviceContext<'_>,
            o: &OrderReference,
        ) -> Result<(), AdapterError> {
            self.detail.find_and_take_order(d, o).await
        }
        async fn pay_hold_invoice(
            &self,
            d: &DeviceContext<'_>,
            o: &OrderReference,
            p: mortsom_app_contract::PaymentPath,
        ) -> Result<mortsom_app_contract::PaymentHandoff, AdapterError> {
            self.detail.pay_hold_invoice(d, o, p).await
        }
        async fn provide_buyer_invoice(
            &self,
            d: &DeviceContext<'_>,
            o: &OrderReference,
            i: Option<&mortsom_common::Secret>,
        ) -> Result<Option<mortsom_common::Secret>, AdapterError> {
            self.detail.provide_buyer_invoice(d, o, i).await
        }
        async fn confirm_fiat_sent(
            &self,
            d: &DeviceContext<'_>,
            o: &OrderReference,
        ) -> Result<(), AdapterError> {
            self.detail.confirm_fiat_sent(d, o).await
        }
        async fn release(
            &self,
            d: &DeviceContext<'_>,
            o: &OrderReference,
        ) -> Result<(), AdapterError> {
            self.detail.release(d, o).await
        }
        async fn cancel_if_open(
            &self,
            d: &DeviceContext<'_>,
            o: &OrderReference,
        ) -> Result<mortsom_app_contract::CancelOutcome, AdapterError> {
            self.detail.cancel_if_open(d, o).await
        }
        async fn visible_actions(
            &self,
            d: &DeviceContext<'_>,
            o: &OrderReference,
        ) -> Result<Vec<String>, AdapterError> {
            self.detail.visible_actions(d, o).await
        }
        async fn cancel_payment(
            &self,
            d: &DeviceContext<'_>,
            o: &OrderReference,
        ) -> Result<(), AdapterError> {
            self.detail.cancel_payment(d, o).await
        }
        /// The form on screen: a fixed waiting state, whatever happened.
        async fn observe_order(
            &self,
            _: &DeviceContext<'_>,
            _: &OrderReference,
        ) -> Result<AppOrderState, AdapterError> {
            Ok(AppOrderState {
                state: OrderState::Taken,
                raw_label: "waiting-payment".into(),
            })
        }
        async fn observe_order_detail(
            &self,
            d: &DeviceContext<'_>,
            o: &OrderReference,
        ) -> Result<AppOrderState, AdapterError> {
            *self.detail_reads.lock().unwrap() += 1;
            self.detail.observe_order(d, o).await
        }
    }

    #[tokio::test]
    async fn a_state_that_leaves_the_take_is_read_from_the_detail_not_the_open_form() {
        let adapter = Arc::new(FormOpen {
            detail: StateAdapter(Mutex::new(OrderState::Pending)),
            detail_reads: Mutex::new(0),
        });
        let ctx = context(adapter.clone());
        let order = OrderReference::new("o1");

        let raw = wait_app_state(&ctx, Actor::Alice, &order, OrderState::Pending)
            .await
            .unwrap();

        assert_eq!(raw, "pending");
        assert!(*adapter.detail_reads.lock().unwrap() > 0);
    }

    #[tokio::test]
    async fn a_state_the_form_flows_into_keeps_reading_the_form() {
        let adapter = Arc::new(FormOpen {
            detail: StateAdapter(Mutex::new(OrderState::Active)),
            detail_reads: Mutex::new(0),
        });
        let ctx = context(adapter.clone());
        let order = OrderReference::new("o1");

        let raw = wait_app_state(&ctx, Actor::Alice, &order, OrderState::Taken)
            .await
            .unwrap();

        assert_eq!(raw, "waiting-payment");
        assert_eq!(*adapter.detail_reads.lock().unwrap(), 0);
    }
}
