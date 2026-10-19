#![allow(clippy::unwrap_used)]
use super::*;
use atspi::zbus::zvariant::OwnedObjectPath;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

type Reference = (String, OwnedObjectPath);

const ROOT: &str = "/org/a11y/atspi/accessible/root";

fn reference(path: &str) -> Reference {
    (":1.1".into(), OwnedObjectPath::try_from(path).unwrap())
}

/// Child lookups being answered at once, and the most there ever were.
#[derive(Default)]
struct InFlight {
    now: AtomicUsize,
    most: AtomicUsize,
}

impl InFlight {
    async fn answer(&self, latency: Duration) {
        let now = self.now.fetch_add(1, Ordering::SeqCst) + 1;
        self.most.fetch_max(now, Ordering::SeqCst);
        tokio::time::sleep(latency).await;
        self.now.fetch_sub(1, Ordering::SeqCst);
    }
}

/// An accessible as at-spi2-atk 2.38 serves it: an occupied socket (Flutter's
/// view) counts and indexes its embedded plug, but leaves it out of
/// GetChildren.
struct Accessible {
    name: String,
    children: Vec<Reference>,
    embedded: Option<Reference>,
    /// How long each child lookup takes to answer.
    latency: Duration,
    served: Arc<InFlight>,
    /// Lookups still to answer with children that are already gone.
    stale: Arc<AtomicUsize>,
}

impl Accessible {
    async fn listed(&self) -> Vec<Reference> {
        self.served.answer(self.latency).await;
        let left = self.stale.load(Ordering::SeqCst);
        let gone = left > 0
            && self
                .stale
                .compare_exchange(left, left - 1, Ordering::SeqCst, Ordering::SeqCst)
                .is_ok();
        let children = self.embedded.iter().chain(&self.children);
        if gone {
            children.map(|_| reference("/gone")).collect()
        } else {
            children.cloned().collect()
        }
    }
}

#[zbus::interface(name = "org.a11y.atspi.Accessible")]
impl Accessible {
    #[zbus(property)]
    fn name(&self) -> String {
        self.name.clone()
    }
    #[zbus(property)]
    fn child_count(&self) -> i32 {
        (self.children.len() + usize::from(self.embedded.is_some())) as i32
    }
    async fn get_children(&self) -> Vec<Reference> {
        let mut listed = self.listed().await;
        if self.embedded.is_some() {
            listed.remove(0);
        }
        listed
    }
    async fn get_child_at_index(&self, index: i32) -> Reference {
        self.listed()
            .await
            .into_iter()
            .nth(index as usize)
            .unwrap_or_else(|| reference("/org/a11y/atspi/null"))
    }
}

async fn serve(objects: Vec<(&str, Accessible)>) -> (Connection, Connection) {
    let (client, server) = tokio::net::UnixStream::pair().unwrap();
    let guid = zbus::Guid::generate();
    let mut builder = zbus::connection::Builder::unix_stream(server)
        .server(guid)
        .unwrap()
        .p2p();
    for (path, object) in objects {
        builder = builder.serve_at(path, object).unwrap();
    }
    let client = zbus::connection::Builder::unix_stream(client).p2p().build();
    let (server, client) = tokio::try_join!(builder.build(), client).unwrap();
    (server, client)
}

fn object(name: &str, children: &[&str], embedded: Option<&str>) -> Accessible {
    Accessible {
        name: name.into(),
        children: children.iter().map(|path| reference(path)).collect(),
        embedded: embedded.map(reference),
        latency: Duration::ZERO,
        served: Arc::default(),
        stale: Arc::default(),
    }
}

#[tokio::test]
async fn reads_the_flutter_tree_embedded_in_an_accessibility_socket() {
    let (_server, client) = serve(vec![
        (ROOT, object("main", &["/app"], None)),
        ("/app", object("foundation.mostro.app", &["/view"], None)),
        ("/view", object("", &[], Some("/plug"))),
        ("/plug", object("", &["/marker"], None)),
        ("/marker", object("[mortsom:env.marker]TEST", &[], None)),
    ])
    .await;
    let native = Native::from_connection(client);
    let marker = native.find("env.marker").await.unwrap();
    assert_eq!(marker.map(|node| node.path), Some("/marker".into()));
}

/// `find` answers the first match and `ui.xml` lists the nodes in walk
/// order: the last child first, each subtree whole before its earlier
/// siblings.
#[tokio::test]
async fn walks_the_tree_in_the_order_it_always_has() {
    let (_server, client) = serve(vec![
        (ROOT, object("main", &["/app"], None)),
        ("/app", object("foundation.mostro.app", &["/window"], None)),
        ("/window", object("", &["/view", "/bar"], None)),
        ("/view", object("", &[], Some("/plug"))),
        ("/plug", object("", &["/a", "/b", "/c"], None)),
        ("/a", object("[mortsom:a]", &["/a1", "/a2"], None)),
        ("/a1", object("[mortsom:dup]deepest", &[], None)),
        ("/a2", object("[mortsom:a2]", &[], None)),
        ("/b", object("[mortsom:b]", &[], None)),
        ("/c", object("", &["/c1"], None)),
        ("/c1", object("[mortsom:c1]", &[], None)),
        ("/bar", object("[mortsom:dup]walked first", &[], None)),
    ])
    .await;
    let native = Native::from_connection(client);

    let nodes = native.nodes().await.unwrap();
    let dup = native.find("dup").await.unwrap().unwrap();

    let paths: Vec<&str> = nodes.iter().map(|node| node.path.as_str()).collect();
    assert_eq!(
        paths,
        [
            ROOT, "/app", "/window", "/bar", "/view", "/plug", "/c", "/c1", "/b", "/a", "/a2",
            "/a1"
        ]
    );
    assert_eq!(nodes[3].name, "[mortsom:dup]walked first");
    assert_eq!(nodes[11].name, "[mortsom:dup]deepest");
    assert_eq!(dup.path, "/bar");
}

/// A busy app answers one call per turn of its main loop. A walk that waits
/// for each answer before asking the next pays a turn per call; one that
/// keeps several out pays far fewer.
#[tokio::test]
async fn keeps_several_calls_out_at_once() {
    let served = Arc::new(InFlight::default());
    let slow = |name: &str, children: &[&str], embedded| Accessible {
        latency: Duration::from_millis(20),
        served: Arc::clone(&served),
        ..object(name, children, embedded)
    };
    let (_server, client) = serve(vec![
        (ROOT, slow("main", &["/app"], None)),
        (
            "/app",
            slow("foundation.mostro.app", &["/x", "/y", "/z"], None),
        ),
        ("/x", slow("", &[], Some("/plug"))),
        ("/y", slow("[mortsom:y]", &[], None)),
        ("/z", slow("[mortsom:z]", &[], None)),
        ("/plug", slow("[mortsom:plug]", &[], None)),
    ])
    .await;

    let nodes = Native::from_connection(client).nodes().await.unwrap();

    assert_eq!(nodes.len(), 6);
    let most = served.most.load(Ordering::SeqCst);
    assert!(most > 1, "at most {most} call answered at a time");
}

/// The limit counts every node the walk reads: 5000 pass, one more is a
/// tree the walk refuses.
#[tokio::test]
async fn holds_the_node_limit_where_it_was() {
    let leaves = |count| vec!["/leaf"; count];
    let (_server, client) = serve(vec![
        (ROOT, object("main", &leaves(4999), None)),
        ("/leaf", object("", &[], None)),
    ])
    .await;
    let read = Native::from_connection(client).nodes().await.unwrap();
    assert_eq!(read.len(), 5000);

    let (_server, client) = serve(vec![
        (ROOT, object("main", &leaves(5000), None)),
        ("/leaf", object("", &[], None)),
    ])
    .await;
    let refused = Native::from_connection(client).nodes().await.unwrap_err();
    assert!(refused.to_string().contains("exceeds limit"), "{refused}");
}

/// A child gone by the time it is read (a route changing under the walk) is
/// not an error: the walk starts over and reads the screen as it now is.
#[tokio::test]
async fn walks_again_when_a_node_vanishes_under_it() {
    let stale = Arc::new(AtomicUsize::new(1));
    let (_server, client) = serve(vec![
        (
            ROOT,
            Accessible {
                stale: Arc::clone(&stale),
                ..object("main", &["/marker"], None)
            },
        ),
        ("/marker", object("[mortsom:env.marker]TEST", &[], None)),
    ])
    .await;

    let marker = Native::from_connection(client)
        .find("env.marker")
        .await
        .unwrap();

    assert_eq!(
        stale.load(Ordering::SeqCst),
        0,
        "the vanished child was read"
    );
    assert_eq!(marker.map(|node| node.path), Some("/marker".into()));
}

#[tokio::test]
async fn finds_nothing_for_an_identifier_not_on_screen() {
    let (_server, client) = serve(vec![
        (ROOT, object("main", &["/marker"], None)),
        ("/marker", object("[mortsom:env.marker]TEST", &[], None)),
    ])
    .await;

    let found = Native::from_connection(client)
        .find("order.status")
        .await
        .unwrap();

    assert!(found.is_none());
}

/// A button as Flutter serves it: no action while it is disabled.
struct Button {
    /// Action-count reads still to answer with none; the button is enabled
    /// after them.
    disabled_reads: Arc<AtomicUsize>,
    pressed: Arc<AtomicUsize>,
    /// How long the app takes to answer a press it already received.
    press_latency: Duration,
}

#[zbus::interface(name = "org.a11y.atspi.Action")]
impl Button {
    #[zbus(property)]
    fn n_actions(&self) -> i32 {
        let left = self.disabled_reads.load(Ordering::SeqCst);
        if left == 0 {
            return 1;
        }
        self.disabled_reads.fetch_sub(1, Ordering::SeqCst);
        0
    }
    async fn do_action(&self, _index: i32) -> bool {
        self.pressed.fetch_add(1, Ordering::SeqCst);
        tokio::time::sleep(self.press_latency).await;
        true
    }
}

/// A screen whose only control, `order.take.confirm`, is `button`.
async fn screen_with(button: Button) -> (Connection, Connection) {
    let (client, server) = tokio::net::UnixStream::pair().unwrap();
    let builder = zbus::connection::Builder::unix_stream(server)
        .server(zbus::Guid::generate())
        .unwrap()
        .p2p()
        .serve_at(ROOT, object("main", &["/take"], None))
        .unwrap()
        .serve_at(
            "/take",
            object("[mortsom:order.take.confirm]Take", &[], None),
        )
        .unwrap()
        .serve_at("/take", button)
        .unwrap();
    let client = zbus::connection::Builder::unix_stream(client).p2p().build();
    let (server, client) = tokio::try_join!(builder.build(), client).unwrap();
    (server, client)
}

/// A control the app has not enabled yet exposes no action: the buyer's
/// Take waits for the payout address (2026-10-19, run
/// `20261019T201137Z-22b7ea1f2a59`). The tap waits for it, as a person
/// would, and presses it once.
#[tokio::test]
async fn a_tap_waits_for_a_control_the_app_has_not_enabled_yet() {
    let pressed = Arc::new(AtomicUsize::new(0));
    let (_server, client) = screen_with(Button {
        disabled_reads: Arc::new(AtomicUsize::new(3)),
        pressed: Arc::clone(&pressed),
        press_latency: Duration::ZERO,
    })
    .await;
    let native = Native::from_connection(client);
    let take = native.find("order.take.confirm").await.unwrap().unwrap();

    native.click(&take).await.unwrap();

    assert_eq!(pressed.load(Ordering::SeqCst), 1);
}

/// The deadline bounds the wait for a control that takes actions, never
/// the press: one cut off after the app received it may still run there,
/// and the run would report a failure for an order it created.
#[tokio::test]
async fn a_press_that_started_is_awaited_past_the_deadline() {
    let pressed = Arc::new(AtomicUsize::new(0));
    let (_server, client) = screen_with(Button {
        disabled_reads: Arc::new(AtomicUsize::new(3)),
        pressed: Arc::clone(&pressed),
        press_latency: Duration::from_millis(600),
    })
    .await;
    let native = Native::from_connection(client);
    let take = native.find("order.take.confirm").await.unwrap().unwrap();

    let pressed_once = native.click_within(&take, Duration::from_millis(400)).await;

    assert_eq!(pressed.load(Ordering::SeqCst), 1);
    assert!(pressed_once.is_ok(), "{pressed_once:?}");
}
