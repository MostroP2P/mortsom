#![allow(clippy::unwrap_used, clippy::expect_used)]

use mostro_core::prelude::{NOSTR_INFO_EVENT_KIND, NOSTR_ORDER_EVENT_KIND};
use nostr_sdk::prelude::{EventBuilder, FinalizeEvent, Keys, Kind, Tag};

use super::*;

fn event(keys: &Keys, kind: Kind, tags: &[&[&str]]) -> Event {
    EventBuilder::new(kind, "content")
        .tags(tags.iter().map(|t| Tag::parse(t.iter().copied()).unwrap()))
        .finalize(keys)
        .unwrap()
}

fn observer(scope: ObserverScope) -> RelayObserver {
    let (tx, _) = broadcast::channel(16);
    RelayObserver {
        inner: Arc::new(Inner {
            client: Client::default(),
            live: true,
            scope: Mutex::new(scope),
            log: Mutex::new(Vec::new()),
            seen: Mutex::new(HashSet::new()),
            subscriptions: Mutex::new(Vec::new()),
            refresh: tokio::sync::Mutex::new(()),
            tx,
        }),
        task: Arc::new(Mutex::new(None)),
    }
}

#[test]
fn scope_filters_grow_with_orders_and_actors() {
    let daemon = Keys::generate();
    let mut scope = ObserverScope::daemon_only(daemon.public_key());
    assert_eq!(scope.filters().len(), 3);

    scope.add_order("o1");
    scope.add_order("o1");
    scope.add_actor(Keys::generate().public_key());
    assert_eq!(scope.order_ids, vec!["o1"]);
    assert_eq!(scope.filters().len(), 6);
}

#[test]
fn scope_matches_only_run_evidence() {
    let daemon = Keys::generate();
    let alice = Keys::generate();
    let stranger = Keys::generate();
    let mut scope = ObserverScope::daemon_only(daemon.public_key());
    scope.add_order("run-order");
    scope.add_actor(alice.public_key());

    let info = event(&daemon, Kind::Custom(NOSTR_INFO_EVENT_KIND), &[]);
    let my_order = event(
        &daemon,
        Kind::Custom(NOSTR_ORDER_EVENT_KIND),
        &[&["d", "run-order"]],
    );
    let other_order = event(
        &daemon,
        Kind::Custom(NOSTR_ORDER_EVENT_KIND),
        &[&["d", "someone-else"]],
    );
    let dm_to_alice = event(
        &daemon,
        Kind::PrivateDirectMessage,
        &[&["p", &alice.public_key().to_hex()]],
    );
    let dm_from_alice = event(&alice, Kind::PrivateDirectMessage, &[]);
    let dm_stranger = event(
        &stranger,
        Kind::PrivateDirectMessage,
        &[&["p", &stranger.public_key().to_hex()]],
    );
    let stranger_info = event(&stranger, Kind::Custom(NOSTR_INFO_EVENT_KIND), &[]);

    assert!(scope.matches(&info));
    assert!(scope.matches(&my_order));
    assert!(!scope.matches(&other_order));
    assert!(scope.matches(&dm_to_alice));
    assert!(scope.matches(&dm_from_alice));
    assert!(!scope.matches(&dm_stranger));
    assert!(!scope.matches(&stranger_info));

    // A trade-key message to the daemon is run evidence on the run relay.
    let dm_to_daemon = event(
        &stranger,
        Kind::PrivateDirectMessage,
        &[&["p", &daemon.public_key().to_hex()]],
    );
    assert!(scope.matches(&dm_to_daemon));
}

#[test]
fn actor_messages_since_counts_only_non_daemon_dms_after_baseline() {
    let daemon = Keys::generate();
    let trade_key = Keys::generate();
    let obs = observer(ObserverScope::daemon_only(daemon.public_key()));
    // Distinct tags keep the two messages from sharing an event id.
    let to_daemon = |keys: &Keys, nonce: &str| {
        event(
            keys,
            Kind::PrivateDirectMessage,
            &[&["p", &daemon.public_key().to_hex()], &["nonce", nonce]],
        )
    };
    assert_eq!(obs.last_seq(), 0);
    obs.ingest("r", &to_daemon(&trade_key, "1"));
    let baseline = obs.last_seq();
    assert_eq!(baseline, 1);
    obs.ingest("r", &event(&daemon, Kind::PrivateDirectMessage, &[]));
    assert!(obs.actor_messages_since(baseline).is_empty());
    obs.ingest("r", &to_daemon(&trade_key, "2"));
    let after = obs.actor_messages_since(baseline);
    assert_eq!(after.len(), 1);
    assert_eq!(after[0].seq, 3);
    assert!(after[0].content.is_none() && after[0].raw.is_none());
}

#[test]
fn ingest_dedupes_sequences_and_drops_encrypted_content() {
    let daemon = Keys::generate();
    let alice = Keys::generate();
    let mut scope = ObserverScope::daemon_only(daemon.public_key());
    scope.add_actor(alice.public_key());
    let observer = observer(scope);
    let info = event(
        &daemon,
        Kind::Custom(NOSTR_INFO_EVENT_KIND),
        &[&["mostro_version", "1"]],
    );
    let dm = event(&alice, Kind::PrivateDirectMessage, &[]);

    let first = observer.ingest("ws://r", &info).unwrap();
    assert!(observer.ingest("ws://r", &info).is_none(), "duplicate");
    let second = observer.ingest("ws://r", &dm).unwrap();

    assert_eq!(first.seq, 1);
    assert_eq!(second.seq, 2);
    assert_eq!(first.content.as_deref(), Some("content"));
    assert_eq!(first.tag("mostro_version"), Some("1"));
    assert_eq!(second.content, None, "kind 14 content is never stored");
    assert!(second.raw.is_none(), "kind 14 is never replayable");
    assert!(first.raw.as_deref().is_some_and(|r| r.contains("\"sig\"")));
    assert_eq!(observer.snapshot().len(), 2);
    assert!(
        observer
            .ingest("ws://r", &event(&Keys::generate(), Kind::TextNote, &[]))
            .is_none()
    );
}

#[tokio::test]
async fn wait_for_finds_past_and_future_events_and_times_out() {
    let daemon = Keys::generate();
    let observer = observer(ObserverScope::daemon_only(daemon.public_key()));
    let info = event(&daemon, Kind::Custom(NOSTR_INFO_EVENT_KIND), &[]);
    observer.ingest("ws://r", &info);

    let past = observer
        .wait_for(Duration::from_millis(10), |e| {
            e.kind == NOSTR_INFO_EVENT_KIND
        })
        .await;
    assert!(past.is_some());

    let waiter = observer.clone();
    let future = tokio::spawn(async move {
        waiter
            .wait_for(Duration::from_secs(5), |e| e.kind == 10002)
            .await
    });
    tokio::task::yield_now().await;
    observer.ingest(
        "ws://r",
        &event(&daemon, Kind::RelayList, &[&["r", "ws://x"]]),
    );
    assert_eq!(future.await.unwrap().unwrap().kind, 10002);

    let none = observer
        .wait_for(Duration::from_millis(20), |e| e.kind == 0)
        .await;
    assert!(none.is_none());
}

#[tokio::test]
async fn start_fails_closed_on_invalid_or_unreachable_relays() {
    let daemon = Keys::generate();
    let scope = ObserverScope::daemon_only(daemon.public_key());

    let invalid = RelayObserver::start(
        &["not a url".into()],
        scope.clone(),
        Duration::from_millis(200),
    )
    .await;
    assert!(matches!(invalid, Err(ObserverError::Relay { .. })));

    let unreachable = RelayObserver::start(
        &["ws://127.0.0.1:1".into()],
        scope,
        Duration::from_millis(300),
    )
    .await;
    assert!(matches!(unreachable, Err(ObserverError::NotConnected(_))));
}

#[tokio::test]
async fn resubscribe_replaces_ids_instead_of_accumulating() {
    let daemon = Keys::generate();
    let observer = observer(ObserverScope::daemon_only(daemon.public_key()));
    // A relay must be registered for subscriptions to exist; it need not
    // be reachable, the client queues the REQ until connected.
    observer
        .inner
        .client
        .add_relay(RelayUrl::parse("ws://127.0.0.1:1").unwrap())
        .await
        .unwrap();

    observer.resubscribe().await.unwrap();
    let first = observer.subscription_ids();
    observer.add_order("o1").await.unwrap();
    let second = observer.subscription_ids();
    observer.add_order("o2").await.unwrap();
    let third = observer.subscription_ids();

    assert_eq!(first.len(), 3);
    assert_eq!(second.len(), 4, "daemon + orders");
    assert_eq!(third.len(), 4, "same filter groups, refreshed in place");
    assert_eq!(second, third);
    assert!(
        third
            .iter()
            .all(|id| id.to_string().starts_with("mortsom-observer-"))
    );
}

#[tokio::test]
async fn detached_observer_scopes_orders_without_a_relay() {
    let daemon = Keys::generate();
    let observer = RelayObserver::detached(ObserverScope::daemon_only(daemon.public_key()));
    let order = event(
        &daemon,
        Kind::Custom(NOSTR_ORDER_EVENT_KIND),
        &[&["d", "o1"]],
    );
    assert!(
        observer.ingest("ws://r", &order).is_none(),
        "out of scope before add_order"
    );

    observer.add_order("o1").await.unwrap();

    assert!(observer.ingest("ws://r", &order).is_some());
    assert!(observer.subscription_ids().is_empty());
    observer.stop().await;
}
