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
