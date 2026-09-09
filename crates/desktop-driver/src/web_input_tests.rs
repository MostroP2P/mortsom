#![allow(clippy::unwrap_used)]
use super::*;
use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{method, path},
};

async fn fixture() -> (MockServer, Session) {
    let server = MockServer::start().await;
    let session = Session::attach(&server.uri(), "test", Duration::from_secs(1)).unwrap();
    Mock::given(method("POST"))
        .and(path("/session/test/element"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"value":{
            "element-6066-11e4-a52e-4f735466cecf":"field"}})))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"value":null})))
        .with_priority(10)
        .mount(&server)
        .await;
    (server, session)
}

#[tokio::test]
async fn editable_text_reads_the_value_property_instead_of_rendered_text() {
    let (server, session) = fixture().await;
    Mock::given(method("GET"))
        .and(path("/session/test/element/field/property/value"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"value":"100"})))
        .mount(&server)
        .await;
    assert_eq!(read(&session, "amount").await.unwrap(), "100");
}

#[tokio::test]
async fn acknowledged_but_ignored_edits_fail_and_never_expose_the_submitted_value() {
    let (server, session) = fixture().await;
    Mock::given(method("GET"))
        .and(path("/session/test/element/field/property/value"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"value":"unchanged"})))
        .mount(&server)
        .await;
    let failure = enter(
        &session,
        "amount",
        "private-entry",
        Duration::from_millis(300),
    )
    .await
    .unwrap_err();
    assert!(!failure.to_string().contains("private-entry"));
    assert!(failure.to_string().contains("Web automation"));
    assert!(!failure.to_string().contains("Linux accessibility"));
    let requests = server.received_requests().await.unwrap();
    assert!(requests.iter().any(|r| r.url.path().ends_with("/clear")));
    assert!(
        requests
            .iter()
            .any(|r| r.url.path().ends_with("/execute/sync"))
    );
}

#[tokio::test]
async fn permanent_driver_errors_propagate_without_retrying_or_echoing_input() {
    let server = MockServer::start().await;
    let session = Session::attach(&server.uri(), "test", Duration::from_secs(1)).unwrap();
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(404).set_body_json(json!({"value":{
            "error":"invalid session id","message":"private-entry"}})))
        .expect(1)
        .mount(&server)
        .await;
    let failure = enter(&session, "amount", "private-entry", Duration::from_secs(1))
        .await
        .unwrap_err();
    assert!(!failure.to_string().contains("private-entry"));
    server.verify().await;
}

#[tokio::test]
async fn refinds_fields_after_scroll_focus_and_clear_and_retries_stale_references() {
    check_rebuilt_field("stale element reference").await;
}

#[tokio::test]
async fn waits_for_an_offscreen_field_to_become_interactable() {
    check_rebuilt_field("element not interactable").await;
}

async fn check_rebuilt_field(code: &'static str) {
    use std::sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    };
    let server = MockServer::start().await;
    let session = Session::attach(&server.uri(), "test", Duration::from_secs(1)).unwrap();
    let finds = Arc::new(AtomicUsize::new(0));
    let written = Arc::new(AtomicBool::new(false));
    let stale = Arc::new(AtomicBool::new(true));
    let counter = finds.clone();
    Mock::given(method("POST"))
        .and(path("/session/test/element"))
        .respond_with(move |_: &wiremock::Request| {
            let id = counter.fetch_add(1, Ordering::SeqCst) + 1;
            ResponseTemplate::new(200).set_body_json(json!({"value":{
                "element-6066-11e4-a52e-4f735466cecf":format!("field{id}")}}))
        })
        .mount(&server)
        .await;
    let flag = written.clone();
    Mock::given(method("GET"))
        .respond_with(move |_: &wiremock::Request| {
            ResponseTemplate::new(200)
                .set_body_json(json!({"value":if flag.load(Ordering::SeqCst) {"100"} else {""}}))
        })
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .respond_with(move |request: &wiremock::Request| {
            if request.url.path().ends_with("/click") && stale.swap(false, Ordering::SeqCst) {
                return ResponseTemplate::new(404).set_body_json(json!({"value":{
                "error":code,"message":"rebuilt"}}));
            }
            if request.url.path().ends_with("/value") {
                written.store(true, Ordering::SeqCst);
            }
            ResponseTemplate::new(200).set_body_json(json!({"value":null}))
        })
        .with_priority(10)
        .mount(&server)
        .await;
    enter(&session, "amount", "100", Duration::from_secs(1))
        .await
        .unwrap();
    let requests = server.received_requests().await.unwrap();
    let mut last_lookup = 0;
    let mut acted = 0;
    for request in requests {
        let path = request.url.path();
        if path.ends_with("/element") {
            last_lookup += 1;
        }
        if ["/click", "/clear", "/value"]
            .iter()
            .any(|suffix| path.ends_with(suffix))
        {
            assert!(
                last_lookup > acted,
                "every editing action re-finds the field"
            );
            assert!(path.contains(&format!("/field{last_lookup}/")));
            acted = last_lookup;
        }
    }
}

fn rect_json(x: f64) -> serde_json::Value {
    json!({"value":{"x":x,"y":10.0,"width":80.0,"height":20.0}})
}

/// The button moves for a moment (a scroll settling, a semantics rebuild),
/// then rests. One click, and only once it rests.
#[tokio::test]
async fn a_press_waits_for_the_node_to_rest_and_clicks_exactly_once() {
    let (server, session) = fixture().await;
    Mock::given(method("GET"))
        .and(path("/session/test/element/field/rect"))
        .respond_with(ResponseTemplate::new(200).set_body_json(rect_json(5.0)))
        .up_to_n_times(1)
        .with_priority(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/session/test/element/field/rect"))
        .respond_with(ResponseTemplate::new(200).set_body_json(rect_json(40.0)))
        .with_priority(5)
        .mount(&server)
        .await;

    press(&session, "submit", Duration::from_secs(2))
        .await
        .unwrap();

    let requests = server.received_requests().await.unwrap();
    let clicks = requests
        .iter()
        .filter(|r| r.url.path().ends_with("/element/field/click"))
        .count();
    assert_eq!(clicks, 1, "{requests:#?}");
    let rects = requests
        .iter()
        .filter(|r| r.url.path().ends_with("/element/field/rect"))
        .count();
    assert!(rects >= 3, "the moving node was re-measured: {rects}");
}

/// A node that never settles is never clicked: an unsent press is a plain
/// failure, a press delivered to the wrong place is not.
#[tokio::test]
async fn a_node_that_never_rests_is_never_clicked() {
    let (server, session) = fixture().await;
    let moving = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
    Mock::given(method("GET"))
        .and(path("/session/test/element/field/rect"))
        .respond_with({
            let moving = moving.clone();
            move |_: &wiremock::Request| {
                let n = moving.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                ResponseTemplate::new(200).set_body_json(rect_json(f64::from(n)))
            }
        })
        .mount(&server)
        .await;

    let failure = press(&session, "submit", Duration::from_millis(600))
        .await
        .unwrap_err();

    assert!(
        failure.to_string().contains("never came to rest"),
        "{failure}"
    );
    let requests = server.received_requests().await.unwrap();
    assert!(!requests.iter().any(|r| r.url.path().ends_with("/click")));
}
