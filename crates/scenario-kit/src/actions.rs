//! Domain actions (spec §7): each finishes only after its expected visible
//! state appears and, when appropriate, its external consequence is
//! observed. Every action records evidence and a timeline entry.

use std::time::Duration;

use mortsom_android_driver::avd::Actor;
use mortsom_app_contract::{OrderDraft, OrderReference, OrderState, PaymentPath};
use mortsom_observers::wallet::{PaymentObservation, PaymentState, payment_hash_of};
use tracing::info;

use crate::context::{Context, ScenarioError};
use crate::timeline::TimelineKind;
use crate::waits::wait_until;

/// Poll interval for state observations.
pub const STATE_POLL: Duration = Duration::from_secs(3);
/// Wire kind of order events.
const ORDER_EVENT_KIND: u16 = 38383;

/// Creates an order as `actor` and registers its cleanup immediately.
pub async fn create_order(
    ctx: &Context,
    actor: Actor,
    draft: &OrderDraft,
) -> Result<OrderReference, ScenarioError> {
    let session = ctx.actor(actor)?;
    let visible = ctx
        .adapter
        .create_order(&session.device(ctx.ui_timeout), draft)
        .await?;
    let id = visible
        .id
        .ok_or_else(|| ScenarioError::product("the app shows no id for the created order"))?;
    let order = OrderReference::with_kind(id.clone(), draft.kind);
    ctx.cleanup.register(actor, &order);
    ctx.relay
        .add_order(&id)
        .await
        .map_err(|e| ScenarioError::relay(format!("cannot scope observer to order {id}: {e}")))?;
    let step = ctx
        .evidence
        .checkpoint(actor.as_str(), "create-order", session.ui.as_ref())
        .await;
    ctx.timeline.push(
        TimelineKind::Step,
        Some(actor.as_str()),
        format!(
            "created {:?} order {id} ({} sats, {} {})",
            draft.kind, draft.amount_sats, draft.fiat_amount, draft.fiat_code
        ),
        Some(step),
    );
    info!(actor = %actor, order = %id, "order created");
    Ok(order)
}

/// Takes `order` as `actor`.
pub async fn take_order(
    ctx: &Context,
    actor: Actor,
    order: &OrderReference,
) -> Result<(), ScenarioError> {
    let session = ctx.actor(actor)?;
    ctx.adapter
        .find_and_take_order(&session.device(ctx.ui_timeout), order)
        .await?;
    let step = ctx
        .evidence
        .checkpoint(actor.as_str(), "take-order", session.ui.as_ref())
        .await;
    ctx.timeline.push(
        TimelineKind::Step,
        Some(actor.as_str()),
        format!("took order {}", order.id),
        Some(step),
    );
    Ok(())
}

/// Waits until `actor`'s app shows `expected` for `order`, bounded by the
/// state timeout. Returns the raw label the app rendered.
pub async fn wait_app_state(
    ctx: &Context,
    actor: Actor,
    order: &OrderReference,
    expected: OrderState,
) -> Result<String, ScenarioError> {
    let session = ctx.actor(actor)?;
    let device = session.device(ctx.ui_timeout);
    let found = wait_until(ctx.state_timeout, STATE_POLL, || async {
        match ctx.adapter.observe_order(&device, order).await {
            Ok(state) if state.state == expected => Some(Ok(state.raw_label)),
            // A terminal state other than the expected one can never become
            // it: fail fast instead of waiting out the deadline.
            Ok(state) if state.state.is_terminal() => Some(Err(ScenarioError::product(format!(
                "order {} reached terminal `{}` while waiting for {expected}",
                order.id, state.raw_label
            )))),
            Ok(_) => None,
            Err(_) => None,
        }
    })
    .await;
    match found {
        Some(Ok(raw)) => {
            let step = ctx
                .evidence
                .checkpoint(
                    actor.as_str(),
                    &format!("state-{expected}"),
                    session.ui.as_ref(),
                )
                .await;
            ctx.timeline.push(
                TimelineKind::UiState,
                Some(actor.as_str()),
                format!("order {} shows {expected} (`{raw}`)", order.id),
                Some(step),
            );
            Ok(raw)
        }
        Some(Err(e)) => Err(e),
        None => {
            ctx.evidence
                .checkpoint(
                    actor.as_str(),
                    &format!("state-{expected}-timeout"),
                    session.ui.as_ref(),
                )
                .await;
            Err(ScenarioError::product(format!(
                "order {} did not reach {expected} on {actor} within {:?}",
                order.id, ctx.state_timeout
            )))
        }
    }
}

/// Waits until both apps show `expected`.
pub async fn wait_both(
    ctx: &Context,
    order: &OrderReference,
    expected: OrderState,
) -> Result<(), ScenarioError> {
    for actor in Actor::ALL {
        wait_app_state(ctx, actor, order, expected).await?;
    }
    Ok(())
}

/// Correlation (spec §8): the run relay must show `order` in `expected`.
pub async fn wait_relay_state(
    ctx: &Context,
    order: &OrderReference,
    expected: OrderState,
) -> Result<(), ScenarioError> {
    let id = order.id.clone();
    let event = ctx
        .relay
        .wait_for(ctx.state_timeout, |e| {
            e.kind == ORDER_EVENT_KIND
                && e.tag("d") == Some(id.as_str())
                && e.tag("s").and_then(OrderState::from_protocol_status) == Some(expected)
        })
        .await
        .ok_or_else(|| {
            ScenarioError::relay(format!(
                "no relay event shows order {} as {expected} within {:?}",
                order.id, ctx.state_timeout
            ))
        })?;
    ctx.timeline.push_at(
        event.received_at_ms,
        TimelineKind::RelayEvent,
        None,
        format!(
            "relay: order {} is {expected} (event {})",
            order.id,
            short(&event.id)
        ),
        Some(event.id.clone()),
    );
    Ok(())
}

/// Pays the hold invoice as `actor` through the app's wallet path, within
/// the caps, and waits for the wallet to report the payment settled.
pub async fn pay_hold_invoice(
    ctx: &Context,
    actor: Actor,
    order: &OrderReference,
) -> Result<PaymentObservation, ScenarioError> {
    ctx.require_mainnet()?;
    let session = ctx.actor(actor)?;
    let reservation = {
        let mut budget = ctx.budget.lock().unwrap_or_else(|p| p.into_inner());
        budget
            .reserve(&order.id, ctx.amount_sats)
            .map_err(|e| ScenarioError::environment(e.to_string()))?
    };
    ctx.timeline.push(
        TimelineKind::Step,
        Some(actor.as_str()),
        format!("reserved {} sats for order {}", ctx.amount_sats, order.id),
        None,
    );
    let handoff = match ctx
        .adapter
        .pay_hold_invoice(&session.device(ctx.ui_timeout), order)
        .await
    {
        Ok(handoff) => handoff,
        Err(e) => {
            release_quietly(ctx, &reservation);
            return Err(e.into());
        }
    };
    let Some(invoice) = handoff.invoice.as_ref() else {
        mark_in_flight(ctx, &reservation, None);
        return Err(ScenarioError::product(
            "the app did not expose the hold invoice; the payment cannot be correlated",
        ));
    };
    ctx.protect(invoice);
    let hash = payment_hash_of(invoice).map_err(|e| ScenarioError::product(e.to_string()))?;
    let step = ctx
        .evidence
        .checkpoint(actor.as_str(), "pay-hold-invoice", session.ui.as_ref())
        .await;
    ctx.timeline.push(
        TimelineKind::Step,
        Some(actor.as_str()),
        format!(
            "paid hold invoice for order {} via {:?} (hash {})",
            order.id,
            handoff.kind,
            short(&hash)
        ),
        Some(step),
    );
    let observation = ctx
        .wallets
        .wait_terminal(&session.wallet_label, &order.id, &hash, ctx.settle_timeout)
        .await;
    match observation {
        Ok(obs) if obs.state == PaymentState::Settled => {
            let mut budget = ctx.budget.lock().unwrap_or_else(|p| p.into_inner());
            budget
                .commit(
                    &reservation,
                    obs.amount_sats,
                    obs.fee_sats,
                    Some(hash.clone()),
                )
                .map_err(|e| ScenarioError::environment(e.to_string()))?;
            ctx.timeline.push_at(
                obs.observed_at * 1000,
                TimelineKind::Wallet,
                Some(actor.as_str()),
                format!(
                    "wallet: {} sats settled (fee {}) for order {}",
                    obs.amount_sats, obs.fee_sats, order.id
                ),
                Some(hash),
            );
            Ok(obs)
        }
        Ok(obs) => {
            release_quietly(ctx, &reservation);
            Err(ScenarioError::wallet(format!(
                "hold invoice payment ended {:?} for order {}",
                obs.state, order.id
            )))
        }
        Err(e) => {
            mark_in_flight(ctx, &reservation, Some(hash));
            Err(ScenarioError::wallet(e.to_string()))
        }
    }
}

/// Generates the buyer invoice as `actor` (no value moves).
pub async fn provide_buyer_invoice(
    ctx: &Context,
    actor: Actor,
    order: &OrderReference,
) -> Result<Option<String>, ScenarioError> {
    let session = ctx.actor(actor)?;
    let invoice = ctx
        .adapter
        .provide_buyer_invoice(&session.device(ctx.ui_timeout), order)
        .await?;
    let hash = match &invoice {
        Some(secret) => {
            ctx.protect(secret);
            Some(payment_hash_of(secret).map_err(|e| ScenarioError::product(e.to_string()))?)
        }
        None => None,
    };
    let step = ctx
        .evidence
        .checkpoint(actor.as_str(), "buyer-invoice", session.ui.as_ref())
        .await;
    ctx.timeline.push(
        TimelineKind::Step,
        Some(actor.as_str()),
        format!(
            "provided buyer invoice for order {} (hash {})",
            order.id,
            hash.as_deref()
                .map(short)
                .unwrap_or_else(|| "unknown".into())
        ),
        Some(step),
    );
    Ok(hash)
}

/// Reports fiat sent as `actor`.
pub async fn confirm_fiat_sent(
    ctx: &Context,
    actor: Actor,
    order: &OrderReference,
) -> Result<(), ScenarioError> {
    let session = ctx.actor(actor)?;
    ctx.adapter
        .confirm_fiat_sent(&session.device(ctx.ui_timeout), order)
        .await?;
    let step = ctx
        .evidence
        .checkpoint(actor.as_str(), "fiat-sent", session.ui.as_ref())
        .await;
    ctx.timeline.push(
        TimelineKind::Step,
        Some(actor.as_str()),
        format!("reported fiat sent for order {}", order.id),
        Some(step),
    );
    Ok(())
}

/// Releases as `actor`.
pub async fn release(
    ctx: &Context,
    actor: Actor,
    order: &OrderReference,
) -> Result<(), ScenarioError> {
    let session = ctx.actor(actor)?;
    ctx.adapter
        .release(&session.device(ctx.ui_timeout), order)
        .await?;
    let step = ctx
        .evidence
        .checkpoint(actor.as_str(), "release", session.ui.as_ref())
        .await;
    ctx.timeline.push(
        TimelineKind::Step,
        Some(actor.as_str()),
        format!("released order {}", order.id),
        Some(step),
    );
    Ok(())
}

/// Settlement correlation: the buyer's wallet must show the payout for
/// `hash` settled.
pub async fn expect_payout_settled(
    ctx: &Context,
    actor: Actor,
    order: &OrderReference,
    hash: &str,
) -> Result<PaymentObservation, ScenarioError> {
    let session = ctx.actor(actor)?;
    let obs = ctx
        .wallets
        .wait_terminal(&session.wallet_label, &order.id, hash, ctx.settle_timeout)
        .await
        .map_err(|e| ScenarioError::wallet(e.to_string()))?;
    if obs.state != PaymentState::Settled {
        return Err(ScenarioError::wallet(format!(
            "payout {} ended {:?}",
            short(hash),
            obs.state
        )));
    }
    ctx.timeline.push_at(
        obs.observed_at * 1000,
        TimelineKind::Wallet,
        Some(actor.as_str()),
        format!(
            "wallet: payout {} sats received for order {}",
            obs.amount_sats, order.id
        ),
        Some(hash.to_owned()),
    );
    Ok(obs)
}

/// Marks the order resolved so cleanup does not try to cancel it.
pub fn mark_resolved(ctx: &Context, order: &OrderReference) {
    ctx.cleanup.resolved(order);
}

fn release_quietly(ctx: &Context, reservation: &crate::budget::Reservation) {
    let mut budget = ctx.budget.lock().unwrap_or_else(|p| p.into_inner());
    if let Err(e) = budget.release(reservation) {
        ctx.timeline.push(
            TimelineKind::Step,
            None,
            format!("budget release failed: {e}"),
            None,
        );
    }
}

fn mark_in_flight(ctx: &Context, reservation: &crate::budget::Reservation, hash: Option<String>) {
    let mut budget = ctx.budget.lock().unwrap_or_else(|p| p.into_inner());
    if let Err(e) = budget.mark_in_flight(reservation, hash) {
        ctx.timeline.push(
            TimelineKind::Step,
            None,
            format!("budget in-flight mark failed: {e}"),
            None,
        );
    }
}

fn short(id: &str) -> String {
    id.chars().take(12).collect()
}

/// Whether a payment path may be observed through the wallet at all.
pub fn is_observable(path: PaymentPath) -> bool {
    matches!(path, PaymentPath::Nwc)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use std::sync::{Arc, Mutex};

    use async_trait::async_trait;
    use mortsom_android_driver::appium::AppiumError;
    use mortsom_android_driver::ui::UiSession;
    use mortsom_app_contract::{
        AdapterError, Apk, AppAdapter, AppOrderState, AppProfile, BuildContext, CancelOutcome,
        CapabilitySet, DeviceContext, Identity, PaymentHandoff, TestEnvironment, VisibleOrder,
        WalletConnection,
    };
    use mortsom_common::{Lane, Secret};
    use mortsom_observers::relay::{ObserverScope, RelayObserver};
    use mortsom_observers::wallet::{WalletObserver, WalletReader};
    use nostr_sdk::prelude::Keys;

    use super::*;
    use crate::budget::{Budget, SpendCaps};
    use crate::cleanup::CleanupRegistry;
    use crate::evidence::NullSink;
    use crate::timeline::Timeline;

    struct StateAdapter(Mutex<OrderState>);

    #[async_trait]
    impl AppAdapter for StateAdapter {
        fn profile(&self) -> AppProfile {
            AppProfile {
                name: "fake".into(),
                package: "p".into(),
            }
        }
        fn capabilities(&self) -> CapabilitySet {
            CapabilitySet::default()
        }
        async fn build(
            &self,
            _: &BuildContext,
            _: &dyn mortsom_android_driver::process::CommandRunner,
        ) -> Result<Apk, AdapterError> {
            Err(AdapterError::infrastructure("no"))
        }
        async fn onboard(
            &self,
            _: &DeviceContext<'_>,
            _: &TestEnvironment,
        ) -> Result<Identity, AdapterError> {
            Err(AdapterError::infrastructure("no"))
        }
        async fn displayed_identity(
            &self,
            _: &DeviceContext<'_>,
        ) -> Result<Identity, AdapterError> {
            Err(AdapterError::infrastructure("no"))
        }
        async fn configure_node_and_relay(
            &self,
            _: &DeviceContext<'_>,
            _: &TestEnvironment,
        ) -> Result<(), AdapterError> {
            Ok(())
        }
        async fn configure_nwc(
            &self,
            _: &DeviceContext<'_>,
            _: &str,
            _: &Secret,
        ) -> Result<WalletConnection, AdapterError> {
            Err(AdapterError::infrastructure("no"))
        }
        async fn create_order(
            &self,
            _: &DeviceContext<'_>,
            _: &OrderDraft,
        ) -> Result<VisibleOrder, AdapterError> {
            Err(AdapterError::infrastructure("no"))
        }
        async fn find_and_take_order(
            &self,
            _: &DeviceContext<'_>,
            _: &OrderReference,
        ) -> Result<(), AdapterError> {
            Ok(())
        }
        async fn pay_hold_invoice(
            &self,
            _: &DeviceContext<'_>,
            _: &OrderReference,
        ) -> Result<PaymentHandoff, AdapterError> {
            Err(AdapterError::infrastructure("no"))
        }
        async fn provide_buyer_invoice(
            &self,
            _: &DeviceContext<'_>,
            _: &OrderReference,
        ) -> Result<Option<Secret>, AdapterError> {
            Ok(None)
        }
        async fn confirm_fiat_sent(
            &self,
            _: &DeviceContext<'_>,
            _: &OrderReference,
        ) -> Result<(), AdapterError> {
            Ok(())
        }
        async fn release(
            &self,
            _: &DeviceContext<'_>,
            _: &OrderReference,
        ) -> Result<(), AdapterError> {
            Ok(())
        }
        async fn cancel_if_open(
            &self,
            _: &DeviceContext<'_>,
            _: &OrderReference,
        ) -> Result<CancelOutcome, AdapterError> {
            Ok(CancelOutcome::NotFound)
        }
        async fn observe_order(
            &self,
            _: &DeviceContext<'_>,
            _: &OrderReference,
        ) -> Result<AppOrderState, AdapterError> {
            let state = *self.0.lock().unwrap();
            Ok(AppOrderState {
                state,
                raw_label: state.as_str().into(),
            })
        }
    }

    struct NoUi;
    #[async_trait]
    impl UiSession for NoUi {
        async fn exists(&self, _: &str) -> Result<bool, AppiumError> {
            Ok(false)
        }
        async fn wait_for(&self, _: &str, _: Duration) -> Result<(), AppiumError> {
            Ok(())
        }
        async fn wait_gone(&self, _: &str, _: Duration) -> Result<(), AppiumError> {
            Ok(())
        }
        async fn tap(&self, _: &str) -> Result<(), AppiumError> {
            Ok(())
        }
        async fn type_text(&self, _: &str, _: &str) -> Result<(), AppiumError> {
            Ok(())
        }
        async fn text_of(&self, _: &str) -> Result<String, AppiumError> {
            Ok(String::new())
        }
        async fn is_enabled(&self, _: &str) -> Result<bool, AppiumError> {
            Ok(true)
        }
        async fn label_of(&self, _: &str) -> Result<String, AppiumError> {
            Ok(String::new())
        }
        async fn screenshot_png(&self) -> Result<Vec<u8>, AppiumError> {
            Ok(vec![])
        }
        async fn page_source(&self) -> Result<String, AppiumError> {
            Ok(String::new())
        }
        async fn background(&self, _: u64) -> Result<(), AppiumError> {
            Ok(())
        }
        async fn terminate_app(&self) -> Result<(), AppiumError> {
            Ok(())
        }
        async fn activate_app(&self) -> Result<(), AppiumError> {
            Ok(())
        }
        async fn clear_app(&self) -> Result<(), AppiumError> {
            Ok(())
        }
        async fn end(&self) -> Result<(), AppiumError> {
            Ok(())
        }
    }

    struct NoWallet;
    #[async_trait]
    impl WalletReader for NoWallet {
        async fn lookup(
            &self,
            _: &str,
        ) -> Result<Option<(u64, u64, mortsom_observers::wallet::PaymentState)>, String> {
            Ok(None)
        }
    }

    fn context(adapter: Arc<dyn AppAdapter>) -> Context {
        Context {
            adapter,
            actors: vec![
                crate::context::ActorSession {
                    actor: Actor::Alice,
                    ui: Arc::new(NoUi),
                    wallet_label: "alice".into(),
                },
                crate::context::ActorSession {
                    actor: Actor::Bob,
                    ui: Arc::new(NoUi),
                    wallet_label: "bob".into(),
                },
            ],
            env: TestEnvironment {
                mostro_pubkey: "x".into(),
                relay_url: "ws://r".into(),
                lane: Lane::Local,
            },
            relay: Arc::new(RelayObserver::detached(ObserverScope::daemon_only(
                Keys::generate().public_key(),
            ))),
            wallets: Arc::new(WalletObserver::new(vec![(
                "alice".into(),
                Arc::new(NoWallet) as Arc<dyn WalletReader>,
            )])),
            budget: Arc::new(Mutex::new(Budget::new(SpendCaps::new(10, 10).unwrap()))),
            cleanup: Arc::new(CleanupRegistry::default()),
            timeline: Arc::new(Timeline::default()),
            evidence: Arc::new(NullSink),
            mainnet: None,
            amount_sats: 5,
            fiat_code: "USD".into(),
            fiat_amount: 1,
            payment_method: "cash".into(),
            ui_timeout: Duration::from_millis(10),
            state_timeout: Duration::from_millis(300),
            settle_timeout: Duration::from_millis(10),
        }
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

        assert!(err.message.contains("terminal `canceled`"));
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

        *adapter.0.lock().unwrap() = OrderState::Taken;
        let raw = wait_app_state(&ctx, Actor::Alice, &order, OrderState::Taken)
            .await
            .unwrap();
        assert_eq!(raw, "taken");
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
}
