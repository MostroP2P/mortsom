//! Recovery and fault-injection actions (spec §8, §9.2, Phase 5): app
//! restart, backgrounding, connectivity loss, rotation, relay restart,
//! replay of already-observed daemon events and the negative observation
//! that an interruption did not make the app repeat a protocol action.
//! Every wait is bounded; nothing here moves value.

use std::time::Duration;

use async_trait::async_trait;
use mortsom_android_driver::avd::Actor;
use mortsom_app_contract::{OrderReference, OrderState};
use mortsom_common::FailureCategory;

use crate::actions::wait_app_state;
use crate::context::{Context, ScenarioError};
use crate::timeline::TimelineKind;
use crate::waits::stays_absent;

/// Poll interval for negative observations.
const NEGATIVE_POLL: Duration = Duration::from_millis(250);

/// Relay-side fault injection the runner implements over the run relay.
#[async_trait]
pub trait RelayControl: Send + Sync {
    /// Restarts the run relay in place and returns once it answers again.
    async fn restart(&self) -> Result<(), String>;
    /// Re-publishes already-signed events (JSON) to the run relay and
    /// returns how many the relay accepted.
    async fn replay(&self, events_json: &[String]) -> Result<usize, String>;
}

/// Relay control that refuses everything (unit tests, dry runs).
#[derive(Debug, Default, Clone, Copy)]
pub struct NoRelayControl;

#[async_trait]
impl RelayControl for NoRelayControl {
    async fn restart(&self) -> Result<(), String> {
        Err("relay control unavailable".into())
    }
    async fn replay(&self, _: &[String]) -> Result<usize, String> {
        Err("relay control unavailable".into())
    }
}

/// A point in the relay log from which actor protocol messages are counted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MessageBaseline(u64);

/// Marks the current end of the relay log.
pub fn message_baseline(ctx: &Context) -> MessageBaseline {
    MessageBaseline(ctx.relay.last_seq())
}

/// Negative observation (spec §8): no actor sent a protocol message to the
/// daemon since `baseline` for the whole `window`. Fails as a product bug
/// when one appears, because an interruption must not repeat an action.
pub async fn expect_no_actor_message(
    ctx: &Context,
    baseline: MessageBaseline,
    window: Duration,
    what: &str,
) -> Result<(), ScenarioError> {
    let offending = stays_absent(window, NEGATIVE_POLL, || async {
        ctx.relay
            .actor_messages_since(baseline.0)
            .into_iter()
            .next()
    })
    .await;
    match offending {
        None => {
            ctx.timeline.push(
                TimelineKind::Assertion,
                None,
                format!("no actor protocol message for {window:?} after {what}"),
                None,
            );
            Ok(())
        }
        Some(event) => Err(ScenarioError::product(format!(
            "unexpected protocol message (kind {} from {}) after {what}",
            event.kind,
            short(&event.author)
        ))),
    }
}

/// Force-stops and relaunches `actor`'s app, then verifies `order` is shown
/// in `expected` again without any protocol message being repeated during
/// `quiet_window`. Returns the label the app rendered after the restart.
pub async fn restart_app(
    ctx: &Context,
    actor: Actor,
    order: &OrderReference,
    expected: OrderState,
    quiet_window: Duration,
) -> Result<String, ScenarioError> {
    let baseline = message_baseline(ctx);
    let session = ctx.actor(actor)?;
    session
        .ui
        .terminate_app()
        .await
        .map_err(infra("force-stop app"))?;
    session
        .ui
        .activate_app()
        .await
        .map_err(infra("relaunch app"))?;
    let step = ctx
        .evidence
        .checkpoint(actor.as_str(), "after-restart", session.ui.as_ref())
        .await;
    ctx.timeline.push(
        TimelineKind::Step,
        Some(actor.as_str()),
        format!(
            "force-stopped and relaunched while order {} was {expected}",
            order.id
        ),
        Some(step),
    );
    let raw = wait_app_state(ctx, actor, order, expected).await?;
    expect_no_actor_message(ctx, baseline, quiet_window, "app restart").await?;
    Ok(raw)
}

/// Sends `actor`'s app to the background for `secs` seconds, brings it back
/// and verifies `order` still shows `expected`, with no repeated message.
pub async fn background_app(
    ctx: &Context,
    actor: Actor,
    secs: u64,
    order: &OrderReference,
    expected: OrderState,
    quiet_window: Duration,
) -> Result<String, ScenarioError> {
    let baseline = message_baseline(ctx);
    let session = ctx.actor(actor)?;
    session
        .ui
        .background(secs)
        .await
        .map_err(infra("background app"))?;
    let step = ctx
        .evidence
        .checkpoint(actor.as_str(), "after-background", session.ui.as_ref())
        .await;
    ctx.timeline.push(
        TimelineKind::Step,
        Some(actor.as_str()),
        format!(
            "backgrounded {secs}s while order {} was {expected}",
            order.id
        ),
        Some(step),
    );
    let raw = wait_app_state(ctx, actor, order, expected).await?;
    expect_no_actor_message(ctx, baseline, quiet_window, "backgrounding").await?;
    Ok(raw)
}

/// Cuts `actor`'s connectivity for `outage`, restores it and verifies the
/// app recovers `order` in `expected`. Connectivity is restored even when
/// the wait fails so a later scenario does not inherit an offline device.
pub async fn network_outage(
    ctx: &Context,
    actor: Actor,
    outage: Duration,
    order: &OrderReference,
    expected: OrderState,
) -> Result<String, ScenarioError> {
    let session = ctx.actor(actor)?;
    session
        .ui
        .set_connectivity(false)
        .await
        .map_err(infra("disable connectivity"))?;
    ctx.timeline.push(
        TimelineKind::Step,
        Some(actor.as_str()),
        format!("connectivity off for {outage:?}"),
        None,
    );
    tokio::time::sleep(outage).await;
    let step = ctx
        .evidence
        .checkpoint(actor.as_str(), "offline", session.ui.as_ref())
        .await;
    let restored = session.ui.set_connectivity(true).await;
    ctx.timeline.push(
        TimelineKind::Step,
        Some(actor.as_str()),
        "connectivity restored",
        Some(step),
    );
    restored.map_err(infra("restore connectivity"))?;
    wait_app_state(ctx, actor, order, expected).await
}

/// Rotates the device to landscape and back, verifying `order` survives
/// both configuration changes in `expected`.
pub async fn rotate_device(
    ctx: &Context,
    actor: Actor,
    order: &OrderReference,
    expected: OrderState,
) -> Result<(), ScenarioError> {
    let session = ctx.actor(actor)?;
    session
        .ui
        .set_orientation(true)
        .await
        .map_err(infra("rotate to landscape"))?;
    let step = ctx
        .evidence
        .checkpoint(actor.as_str(), "landscape", session.ui.as_ref())
        .await;
    ctx.timeline.push(
        TimelineKind::Step,
        Some(actor.as_str()),
        "rotated to landscape",
        Some(step),
    );
    let in_landscape = wait_app_state(ctx, actor, order, expected).await;
    let back = session.ui.set_orientation(false).await;
    // The restore is recorded for what it was, and it is recorded even when
    // the landscape assertion is the error the caller sees: the next
    // scenario would otherwise inherit a rotated device with no trace of it.
    match &back {
        Ok(()) => ctx.timeline.push(
            TimelineKind::Step,
            Some(actor.as_str()),
            "rotated back to portrait",
            None,
        ),
        Err(e) => ctx.timeline.push(
            TimelineKind::Step,
            Some(actor.as_str()),
            format!("could not rotate back to portrait: {e}"),
            None,
        ),
    }
    in_landscape?;
    back.map_err(infra("rotate to portrait"))?;
    wait_app_state(ctx, actor, order, expected)
        .await
        .map(|_| ())
}

/// Restarts the run relay and verifies both apps still show `order` in
/// `expected` once it is back.
pub async fn restart_relay(
    ctx: &Context,
    order: &OrderReference,
    expected: OrderState,
) -> Result<(), ScenarioError> {
    ctx.relay_control
        .restart()
        .await
        .map_err(|e| ScenarioError::relay(format!("relay restart: {e}")))?;
    ctx.timeline
        .push(TimelineKind::Step, None, "run relay restarted", None);
    for actor in Actor::ALL {
        wait_app_state(ctx, actor, order, expected).await?;
    }
    Ok(())
}

/// Which observed daemon events to replay.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Replay {
    /// Every order event observed so far, in order (duplicates).
    Duplicates,
    /// Only the oldest order event (a stale state arriving late).
    Stale,
}

/// Re-publishes daemon events already observed for `order` (spec §9.2:
/// duplicate and stale delivery) and verifies both apps keep showing
/// `expected`. Nothing is forged: only signed events the daemon produced.
pub async fn replay_order_events(
    ctx: &Context,
    order: &OrderReference,
    which: Replay,
    expected: OrderState,
) -> Result<usize, ScenarioError> {
    let mut events: Vec<String> = ctx
        .relay
        .snapshot()
        .into_iter()
        .filter(|e| e.tag("d") == Some(order.id.as_str()))
        .filter_map(|e| e.raw)
        .collect();
    if events.is_empty() {
        return Err(ScenarioError::relay(format!(
            "no replayable daemon event observed for order {}",
            order.id
        )));
    }
    if which == Replay::Stale {
        events.truncate(1);
    }
    let accepted = ctx
        .relay_control
        .replay(&events)
        .await
        .map_err(|e| ScenarioError::relay(format!("replay: {e}")))?;
    ctx.timeline.push(
        TimelineKind::Step,
        None,
        format!(
            "replayed {accepted}/{} {which:?} event(s) for order {}",
            events.len(),
            order.id
        ),
        None,
    );
    for actor in Actor::ALL {
        wait_app_state(ctx, actor, order, expected).await?;
    }
    Ok(accepted)
}

fn infra(
    what: &'static str,
) -> impl Fn(mortsom_android_driver::appium::AppiumError) -> ScenarioError {
    move |e| ScenarioError::new(FailureCategory::TestInfrastructure, format!("{what}: {e}"))
}

fn short(hex: &str) -> &str {
    hex.get(..8).unwrap_or(hex)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use std::sync::{Arc, Mutex};

    use mortsom_observers::relay::ObserverScope;
    use nostr_sdk::prelude::{EventBuilder, FinalizeEvent, Keys, Kind, Tag};

    use super::*;
    use crate::test_support::{StateAdapter, context, context_with};

    /// Relay control that records calls and answers as configured.
    struct FakeRelay {
        restarts: Mutex<u32>,
        replayed: Mutex<Vec<Vec<String>>>,
        fail: bool,
    }

    impl FakeRelay {
        fn new(fail: bool) -> Arc<Self> {
            Arc::new(Self {
                restarts: Mutex::new(0),
                replayed: Mutex::new(Vec::new()),
                fail,
            })
        }
    }

    #[async_trait]
    impl RelayControl for FakeRelay {
        async fn restart(&self) -> Result<(), String> {
            *self.restarts.lock().unwrap() += 1;
            if self.fail {
                Err("docker restart failed".into())
            } else {
                Ok(())
            }
        }
        async fn replay(&self, events: &[String]) -> Result<usize, String> {
            self.replayed.lock().unwrap().push(events.to_vec());
            if self.fail {
                Err("relay rejected".into())
            } else {
                Ok(events.len())
            }
        }
    }

    fn adapter(state: OrderState) -> Arc<StateAdapter> {
        Arc::new(StateAdapter(Mutex::new(state)))
    }

    fn dm_to(daemon: &Keys, nonce: &str) -> nostr_sdk::prelude::Event {
        EventBuilder::new(Kind::PrivateDirectMessage, "x")
            .tags([
                Tag::public_key(daemon.public_key()),
                Tag::parse(["nonce", nonce]).unwrap(),
            ])
            .finalize(&Keys::generate())
            .unwrap()
    }

    #[tokio::test]
    async fn restart_app_recovers_state_and_stays_quiet() {
        let ctx = context(adapter(OrderState::Active));
        let order = OrderReference::new("o1");
        let raw = restart_app(
            &ctx,
            Actor::Alice,
            &order,
            OrderState::Active,
            Duration::from_millis(50),
        )
        .await
        .unwrap();
        assert_eq!(raw, "active");
        let entries = ctx.timeline.entries();
        assert!(
            entries
                .iter()
                .any(|e| e.summary.contains("force-stopped and relaunched"))
        );
        assert!(
            entries
                .iter()
                .any(|e| e.summary.contains("no actor protocol message"))
        );
    }

    #[tokio::test]
    async fn restart_app_fails_when_the_app_repeats_an_action() {
        let ctx = context(adapter(OrderState::Active));
        let order = OrderReference::new("o1");
        let daemon = Keys::generate();
        // A detached observer scoped to a daemon key we control.
        let ctx = Context {
            relay: Arc::new(mortsom_observers::relay::RelayObserver::detached(
                ObserverScope::daemon_only(daemon.public_key()),
            )),
            ..ctx
        };
        ctx.relay.ingest("r", &dm_to(&daemon, "before"));
        let relay = Arc::clone(&ctx.relay);
        let injector = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(20)).await;
            relay.ingest("r", &dm_to(&daemon, "after"));
        });
        let err = restart_app(
            &ctx,
            Actor::Alice,
            &order,
            OrderState::Active,
            Duration::from_millis(200),
        )
        .await
        .unwrap_err();
        injector.await.unwrap();
        assert_eq!(err.category, FailureCategory::Product);
        assert!(err.message.contains("unexpected protocol message"));
    }

    #[tokio::test]
    async fn restart_app_reports_a_state_the_app_lost() {
        let ctx = context(adapter(OrderState::Pending));
        let order = OrderReference::new("o1");
        let err = restart_app(
            &ctx,
            Actor::Bob,
            &order,
            OrderState::Active,
            Duration::from_millis(10),
        )
        .await
        .unwrap_err();
        assert!(err.message.contains("did not reach active"));
    }

    #[tokio::test]
    async fn background_network_and_rotation_verify_the_state_afterwards() {
        let ctx = context(adapter(OrderState::FiatSent));
        let order = OrderReference::new("o1");
        background_app(
            &ctx,
            Actor::Alice,
            1,
            &order,
            OrderState::FiatSent,
            Duration::from_millis(10),
        )
        .await
        .unwrap();
        network_outage(
            &ctx,
            Actor::Bob,
            Duration::from_millis(5),
            &order,
            OrderState::FiatSent,
        )
        .await
        .unwrap();
        rotate_device(&ctx, Actor::Alice, &order, OrderState::FiatSent)
            .await
            .unwrap();
        let summaries: Vec<String> = ctx
            .timeline
            .entries()
            .into_iter()
            .map(|e| e.summary)
            .collect();
        assert!(summaries.iter().any(|s| s.contains("backgrounded 1s")));
        assert!(summaries.iter().any(|s| s == "connectivity restored"));
        assert!(summaries.iter().any(|s| s == "rotated back to portrait"));
    }

    #[tokio::test]
    async fn relay_restart_uses_the_control_and_maps_failures_to_relay() {
        let relay = FakeRelay::new(false);
        let ctx = context_with(adapter(OrderState::Taken), relay.clone());
        let order = OrderReference::new("o1");
        restart_relay(&ctx, &order, OrderState::Taken)
            .await
            .unwrap();
        assert_eq!(*relay.restarts.lock().unwrap(), 1);

        let failing = FakeRelay::new(true);
        let ctx = context_with(adapter(OrderState::Taken), failing);
        let err = restart_relay(&ctx, &order, OrderState::Taken)
            .await
            .unwrap_err();
        assert_eq!(err.category, FailureCategory::Relay);
    }

    #[tokio::test]
    async fn replay_uses_only_observed_signed_events_for_the_order() {
        let relay = FakeRelay::new(false);
        let daemon = Keys::generate();
        let ctx = context_with(adapter(OrderState::Active), relay.clone());
        let ctx = Context {
            relay: Arc::new(mortsom_observers::relay::RelayObserver::detached(
                ObserverScope::daemon_only(daemon.public_key()),
            )),
            ..ctx
        };
        let order = OrderReference::new("o1");
        // Nothing observed yet: refuse rather than forge.
        let err = replay_order_events(&ctx, &order, Replay::Duplicates, OrderState::Active)
            .await
            .unwrap_err();
        assert_eq!(err.category, FailureCategory::Relay);

        ctx.relay.add_order("o1").await.unwrap();
        for status in ["pending", "in-progress", "active"] {
            let event = EventBuilder::new(Kind::Custom(38383), "")
                .tags([Tag::identifier("o1"), Tag::parse(["s", status]).unwrap()])
                .finalize(&daemon)
                .unwrap();
            ctx.relay.ingest("r", &event);
        }
        let n = replay_order_events(&ctx, &order, Replay::Duplicates, OrderState::Active)
            .await
            .unwrap();
        assert_eq!(n, 3);
        let stale = replay_order_events(&ctx, &order, Replay::Stale, OrderState::Active)
            .await
            .unwrap();
        assert_eq!(stale, 1);
        let calls = relay.replayed.lock().unwrap();
        assert_eq!(calls.len(), 2);
        assert!(calls[1][0].contains("\"pending\""), "stale = oldest event");
    }
    /// A device left in landscape poisons the next scenario, so a failed
    /// restore is always visible in the timeline.
    #[tokio::test]
    async fn a_failed_rotation_restore_is_recorded() {
        struct StuckRotation;
        #[async_trait::async_trait]
        impl mortsom_android_driver::ui::UiSession for StuckRotation {
            /// Rotating away works; coming back does not.
            async fn set_orientation(
                &self,
                landscape: bool,
            ) -> Result<(), mortsom_android_driver::appium::AppiumError> {
                if landscape {
                    return Ok(());
                }
                Err(mortsom_android_driver::appium::AppiumError::NotReady {
                    url: "x".into(),
                    timeout: Duration::ZERO,
                })
            }
            async fn exists(
                &self,
                _: &str,
            ) -> Result<bool, mortsom_android_driver::appium::AppiumError> {
                Ok(false)
            }
            async fn wait_for(
                &self,
                _: &str,
                _: Duration,
            ) -> Result<(), mortsom_android_driver::appium::AppiumError> {
                Ok(())
            }
            async fn wait_gone(
                &self,
                _: &str,
                _: Duration,
            ) -> Result<(), mortsom_android_driver::appium::AppiumError> {
                Ok(())
            }
            async fn tap(
                &self,
                _: &str,
            ) -> Result<(), mortsom_android_driver::appium::AppiumError> {
                Ok(())
            }
            async fn type_text(
                &self,
                _: &str,
                _: &str,
            ) -> Result<(), mortsom_android_driver::appium::AppiumError> {
                Ok(())
            }
            async fn text_of(
                &self,
                _: &str,
            ) -> Result<String, mortsom_android_driver::appium::AppiumError> {
                Ok(String::new())
            }
            async fn is_enabled(
                &self,
                _: &str,
            ) -> Result<bool, mortsom_android_driver::appium::AppiumError> {
                Ok(true)
            }
            async fn label_of(
                &self,
                _: &str,
            ) -> Result<String, mortsom_android_driver::appium::AppiumError> {
                Ok(String::new())
            }
            async fn screenshot_png(
                &self,
            ) -> Result<Vec<u8>, mortsom_android_driver::appium::AppiumError> {
                Ok(vec![])
            }
            async fn page_source(
                &self,
            ) -> Result<String, mortsom_android_driver::appium::AppiumError> {
                Ok(String::new())
            }
            async fn background(
                &self,
                _: u64,
            ) -> Result<(), mortsom_android_driver::appium::AppiumError> {
                Ok(())
            }
            async fn terminate_app(
                &self,
            ) -> Result<(), mortsom_android_driver::appium::AppiumError> {
                Ok(())
            }
            async fn activate_app(
                &self,
            ) -> Result<(), mortsom_android_driver::appium::AppiumError> {
                Ok(())
            }
            async fn clear_app(&self) -> Result<(), mortsom_android_driver::appium::AppiumError> {
                Ok(())
            }
            async fn set_connectivity(
                &self,
                _: bool,
            ) -> Result<(), mortsom_android_driver::appium::AppiumError> {
                Ok(())
            }
            async fn end(&self) -> Result<(), mortsom_android_driver::appium::AppiumError> {
                Ok(())
            }
        }

        let adapter: Arc<dyn mortsom_app_contract::AppAdapter> =
            Arc::new(StateAdapter(Mutex::new(OrderState::Active)));
        let mut ctx = context(adapter);
        for session in &mut ctx.actors {
            session.ui = Arc::new(StuckRotation);
        }
        let order = OrderReference::new("o1");

        let err = rotate_device(&ctx, Actor::Alice, &order, OrderState::Active)
            .await
            .unwrap_err();

        assert!(err.message.contains("rotate"), "{err}");
        let summaries: Vec<String> = ctx
            .timeline
            .entries()
            .into_iter()
            .map(|e| e.summary)
            .collect();
        assert!(
            summaries
                .iter()
                .any(|s| s.contains("could not rotate back to portrait")),
            "{summaries:?}"
        );
        assert!(
            !summaries.iter().any(|s| s == "rotated back to portrait"),
            "a failed restore must not be reported as done: {summaries:?}"
        );
    }
}
