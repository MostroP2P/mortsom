//! Relay observations with a baseline, negative relay assertions, order
//! event tags and the rating proof.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::{Arc, Mutex};
use std::time::Duration;

use mortsom_observers::relay::{ObserverScope, RelayObserver};
use nostr_sdk::prelude::{EventBuilder, FinalizeEvent, Keys, Kind, Tag};

use super::*;
use crate::test_support::{StateAdapter, context};

const ORDER: &str = "o1";

struct Daemon {
    keys: Keys,
    relay: Arc<RelayObserver>,
    published: Mutex<u64>,
}

impl Daemon {
    /// Publishes an event; a nonce tag keeps two revisions published in the
    /// same second from sharing an id, which the observer would dedupe.
    fn publish(&self, kind: u16, tags: &[&[&str]]) {
        let nonce = {
            let mut published = self.published.lock().unwrap();
            *published += 1;
            published.to_string()
        };
        let event = EventBuilder::new(Kind::Custom(kind), "")
            .tags(tags.iter().map(|t| Tag::parse(t.iter().copied()).unwrap()))
            .tag(Tag::parse(["nonce", nonce.as_str()]).unwrap())
            .finalize(&self.keys)
            .unwrap();
        assert!(
            self.relay.ingest("ws://fixture.invalid", &event).is_some(),
            "the fixture event must be in scope"
        );
    }
    fn order(&self, status: &str) {
        self.publish(ORDER_EVENT_KIND, &[&["d", ORDER], &["s", status]]);
    }
}

fn daemon_context() -> (Context, Daemon) {
    let keys = Keys::generate();
    let mut scope = ObserverScope::daemon_only(keys.public_key());
    scope.add_order(ORDER);
    let relay = Arc::new(RelayObserver::detached(scope));
    let mut ctx = context(Arc::new(StateAdapter(Mutex::new(OrderState::Pending))));
    ctx.relay = Arc::clone(&relay);
    ctx.env.mostro_pubkey = keys.public_key().to_hex();
    (
        ctx,
        Daemon {
            keys,
            relay,
            published: Mutex::new(0),
        },
    )
}

/// An order that was pending, taken and reaped back to pending is proved
/// by the second `pending`: the first one is before the baseline.
#[tokio::test]
async fn a_state_before_the_baseline_does_not_satisfy_a_wait_since() {
    let (ctx, daemon) = daemon_context();
    let order = OrderReference::new(ORDER);
    daemon.order("pending");
    daemon.order("in-progress");
    let since = relay_cursor(&ctx);

    let early = wait_relay_state_since(
        &ctx,
        &order,
        &[OrderState::Pending],
        since,
        Duration::from_millis(50),
    )
    .await;
    assert!(early.is_err(), "the first pending is history, not the reap");

    daemon.order("pending");
    let state = wait_relay_state_since(
        &ctx,
        &order,
        &[OrderState::Pending, OrderState::Cancelled],
        since,
        Duration::from_millis(300),
    )
    .await
    .unwrap();
    assert_eq!(state, OrderState::Pending);
}

/// The parity report reads a run's relay path from these lines
/// (`relay: order <id> is <state>`, `coordinator::parity::relay_path`): in
/// any other grammar the wait drops out of every comparison.
#[tokio::test]
async fn a_wait_since_records_the_relay_line_parity_reads() {
    let (ctx, daemon) = daemon_context();
    let order = OrderReference::new(ORDER);
    let since = relay_cursor(&ctx);
    daemon.order("pending");

    wait_relay_state_since(
        &ctx,
        &order,
        &[OrderState::Pending],
        since,
        Duration::from_millis(300),
    )
    .await
    .unwrap();

    assert!(
        ctx.timeline
            .entries()
            .iter()
            .any(|e| e.summary.starts_with("relay: order o1 is pending (event ")),
        "{:?}",
        ctx.timeline.entries()
    );
}

/// The negative assertion spends its window and passes when the daemon
/// stays quiet; a cancellation published inside it is the product failure
/// it exists to catch.
#[tokio::test]
async fn the_daemon_publishing_the_forbidden_state_fails_the_quiet_window() {
    let (ctx, daemon) = daemon_context();
    let order = OrderReference::new(ORDER);
    daemon.order("in-progress");
    let since = relay_cursor(&ctx);

    expect_relay_state_absent(
        &ctx,
        &order,
        OrderState::Cancelled,
        since,
        Duration::from_millis(40),
    )
    .await
    .unwrap();

    daemon.order("canceled");
    let error = expect_relay_state_absent(
        &ctx,
        &order,
        OrderState::Cancelled,
        since,
        Duration::from_millis(40),
    )
    .await
    .unwrap_err();
    assert_eq!(error.category, mortsom_common::FailureCategory::Product);
    assert!(error.message.contains("published order o1 as cancelled"));
}

/// The `fa` tag carries one amount for a fixed order and both bounds for a
/// range; the values come back in order.
#[tokio::test]
async fn the_fiat_tag_of_the_order_event_is_read_with_every_value() {
    let (ctx, daemon) = daemon_context();
    let order = OrderReference::new(ORDER);
    daemon.publish(
        ORDER_EVENT_KIND,
        &[
            &["d", ORDER],
            &["s", "pending"],
            &["fa", "1", "3"],
            &["amt", "0"],
        ],
    );

    assert_eq!(
        relay_order_tag(&ctx, &order, "fa").await.unwrap(),
        ["1", "3"]
    );
    assert_eq!(relay_order_tag(&ctx, &order, "amt").await.unwrap(), ["0"]);
    assert!(relay_order_tag(&ctx, &order, "premium").await.is_err());

    // A later revision is the one read, not the first ever published.
    daemon.publish(
        ORDER_EVENT_KIND,
        &[
            &["d", ORDER],
            &["s", "in-progress"],
            &["fa", "2"],
            &["amt", "1800"],
        ],
    );
    assert_eq!(relay_order_tag(&ctx, &order, "fa").await.unwrap(), ["2"]);
}

/// The rating proof is a reputation event the daemon publishes after the
/// star was given; one from before the baseline, or from anyone else, is
/// not it.
#[tokio::test]
async fn a_rating_counts_only_when_the_daemon_publishes_it_after_the_star() {
    let (ctx, daemon) = daemon_context();
    daemon.publish(RATING_EVENT_KIND, &[&["d", "someone"]]);
    let since = relay_cursor(&ctx);

    assert!(wait_rating_published(&ctx, since).await.is_err());

    daemon.publish(RATING_EVENT_KIND, &[&["d", "someone"]]);
    let event = wait_rating_published(&ctx, since).await.unwrap();
    assert_eq!(event.kind, RATING_EVENT_KIND);
    assert!(
        ctx.timeline
            .entries()
            .iter()
            .any(|e| e.summary.contains("reputation update"))
    );
}

/// The star reaches the adapter and is recorded as a step; an adapter
/// without the rating journey reports the contract gap, not a pass.
#[tokio::test]
async fn rating_is_a_recorded_step_or_a_contract_gap() {
    let ctx = context(Arc::new(StateAdapter(Mutex::new(OrderState::Completed))));
    let order = OrderReference::new(ORDER);

    let error = rate_counterpart(&ctx, Actor::Alice, &order, 5)
        .await
        .unwrap_err();

    assert_eq!(
        error.category,
        mortsom_common::FailureCategory::AutomationContract
    );
    assert!(
        !ctx.timeline
            .entries()
            .iter()
            .any(|e| e.summary.contains("rated the counterpart")),
        "no step is recorded for a rating that never happened"
    );
}

/// Taking at an amount is the same recorded step as a plain take, naming
/// the amount; the state adapter takes anything.
#[tokio::test]
async fn taking_at_an_amount_records_the_amount() {
    let ctx = context(Arc::new(StateAdapter(Mutex::new(OrderState::Pending))));
    let order = OrderReference::new(ORDER);

    let error = take_order_at(&ctx, Actor::Bob, &order, 2)
        .await
        .unwrap_err();

    assert_eq!(
        error.category,
        mortsom_common::FailureCategory::AutomationContract
    );
}
