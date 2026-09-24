#![allow(clippy::unwrap_used)]
//! Reads of a node the app replaces between the find and the read: Flutter
//! Web rebuilds the semantics of a screen that redraws, such as the trade
//! detail whose countdown ticks every second.
use super::*;
use serde_json::json;
use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{method, path},
};

const LIMIT: Duration = Duration::from_millis(600);

async fn fixture() -> (MockServer, Session) {
    let server = MockServer::start().await;
    let session = Session::attach(&server.uri(), "test", Duration::from_secs(1)).unwrap();
    Mock::given(method("POST"))
        .and(path("/session/test/element"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"value":{
            "element-6066-11e4-a52e-4f735466cecf":"field"}})))
        .with_priority(10)
        .mount(&server)
        .await;
    (server, session)
}

fn stale() -> ResponseTemplate {
    ResponseTemplate::new(404).set_body_json(json!({"value":{
        "error":"stale element reference","message":"stale element not found in the current frame"}}))
}

/// The first `reads` answers at `route` are stale; later ones are `value`.
async fn replaced_then(server: &MockServer, route: &str, reads: u64, value: serde_json::Value) {
    Mock::given(method("GET"))
        .and(path(route))
        .respond_with(stale())
        .up_to_n_times(reads)
        .with_priority(1)
        .mount(server)
        .await;
    Mock::given(method("GET"))
        .and(path(route))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "value": value })))
        .with_priority(5)
        .mount(server)
        .await;
}

async fn finds(server: &MockServer) -> usize {
    server
        .received_requests()
        .await
        .unwrap()
        .iter()
        .filter(|r| r.method.as_str() == "POST" && r.url.path() == "/session/test/element")
        .count()
}

/// The live failure of `range_buy_order` on Web (2026-09-24): `order.id`
/// read on a trade detail that redrew between the find and the read.
#[tokio::test]
async fn a_label_read_on_a_replaced_node_finds_it_again_and_reads_it() {
    let (server, session) = fixture().await;
    replaced_then(
        &server,
        "/session/test/element/field/attribute/aria-label",
        1,
        json!("62adb67b-e420-42e7-b4e7-dfc208fb2eb5"),
    )
    .await;

    let shown = label(&session, "order.id", LIMIT).await.unwrap();

    assert_eq!(shown, "62adb67b-e420-42e7-b4e7-dfc208fb2eb5");
    assert_eq!(finds(&server).await, 2, "the node was found again");
}

#[tokio::test]
async fn an_enabled_read_on_a_replaced_node_finds_it_again() {
    let (server, session) = fixture().await;
    replaced_then(
        &server,
        "/session/test/element/field/attribute/aria-disabled",
        1,
        json!(null),
    )
    .await;
    Mock::given(method("GET"))
        .and(path("/session/test/element/field/enabled"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"value": true})))
        .mount(&server)
        .await;

    assert!(enabled(&session, "invoice.submit", LIMIT).await.unwrap());
    assert_eq!(finds(&server).await, 2);
}

#[tokio::test]
async fn a_text_read_on_a_replaced_field_finds_it_again() {
    let (server, session) = fixture().await;
    replaced_then(
        &server,
        "/session/test/element/field/property/value",
        1,
        json!("lnbcrt10u1invoice"),
    )
    .await;

    let shown = text(&session, "invoice.text", LIMIT).await.unwrap();

    assert_eq!(shown, "lnbcrt10u1invoice");
    assert_eq!(finds(&server).await, 2);
}

/// A node that is gone when looked for again is gone: the read reports
/// it rather than polling for a control that no longer exists.
#[tokio::test]
async fn a_node_gone_when_found_again_is_reported_not_polled_for() {
    let (server, session) = fixture().await;
    Mock::given(method("GET"))
        .and(path("/session/test/element/field/attribute/aria-label"))
        .respond_with(stale())
        .mount(&server)
        .await;
    // Found once, replaced, and gone when looked for again.
    Mock::given(method("POST"))
        .and(path("/session/test/element"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"value":{
            "element-6066-11e4-a52e-4f735466cecf":"field"}})))
        .up_to_n_times(1)
        .with_priority(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/session/test/element"))
        .respond_with(ResponseTemplate::new(404).set_body_json(json!({"value":{
            "error":"no such element","message":"not found"}})))
        .with_priority(2)
        .mount(&server)
        .await;

    let failure = label(&session, "order.id", LIMIT).await.unwrap_err();

    assert!(
        matches!(&failure, UiError::WebDriver(cause) if cause.is_no_such_element()),
        "{failure}"
    );
    assert_eq!(finds(&server).await, 2, "looked for once more, not polled");
}

/// A node that keeps being replaced is read for at most the limit, then
/// reported, never polled forever.
#[tokio::test]
async fn a_node_replaced_on_every_read_is_reported_at_the_limit() {
    let (server, session) = fixture().await;
    Mock::given(method("GET"))
        .and(path("/session/test/element/field/attribute/aria-label"))
        .respond_with(stale())
        .mount(&server)
        .await;

    let started = std::time::Instant::now();
    let failure = label(&session, "order.id", LIMIT).await.unwrap_err();

    assert!(
        matches!(&failure, UiError::WebDriver(cause) if cause.is_stale_element()),
        "{failure}"
    );
    assert!(
        started.elapsed() < LIMIT * 3,
        "bounded: {:?}",
        started.elapsed()
    );
    assert!(finds(&server).await >= 2);
}

/// Any other failure is the answer: only a replaced node is looked for
/// again.
#[tokio::test]
async fn a_read_failing_for_another_reason_is_not_retried() {
    let (server, session) = fixture().await;
    Mock::given(method("GET"))
        .and(path("/session/test/element/field/attribute/aria-label"))
        .respond_with(ResponseTemplate::new(500).set_body_json(json!({"value":{
            "error":"unknown error","message":"renderer gone"}})))
        .mount(&server)
        .await;

    let failure = label(&session, "order.id", LIMIT).await.unwrap_err();

    assert!(failure.to_string().contains("unknown error"), "{failure}");
    assert_eq!(finds(&server).await, 1);
}
