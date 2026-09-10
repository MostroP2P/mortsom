#![allow(clippy::unwrap_used, clippy::expect_used)]

use wiremock::matchers::{body_partial_json, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use super::*;

const T: Duration = Duration::from_secs(5);

async fn session(server: &MockServer) -> Session {
    Mock::given(method("POST"))
        .and(path("/session"))
        .and(body_partial_json(
            json!({"capabilities": {"alwaysMatch": {"browserName": "chrome"}}}),
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "value": {"sessionId": "s1", "capabilities": {}}
        })))
        .mount(server)
        .await;
    Session::create(&server.uri(), json!({"browserName": "chrome"}), T)
        .await
        .unwrap()
}

#[tokio::test]
async fn create_find_click_type_text_and_delete() {
    let server = MockServer::start().await;
    let s = session(&server).await;
    Mock::given(method("POST"))
        .and(path("/session/s1/element"))
        .and(body_partial_json(
            json!({"using": "accessibility id", "value": "order.create.submit"}),
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "value": {ELEMENT_KEY: "e1"}
        })))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/session/s1/element/e1/click"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"value": null})))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/session/s1/element/e1/value"))
        .and(body_partial_json(json!({"text": "1000"})))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"value": null})))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/session/s1/element/e1/text"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"value": "Pending"})))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/session/s1/element/e1/enabled"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"value": true})))
        .mount(&server)
        .await;
    Mock::given(method("DELETE"))
        .and(path("/session/s1"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"value": null})))
        .mount(&server)
        .await;

    let element = s
        .find_element(Locator::AccessibilityId, "order.create.submit")
        .await
        .unwrap();
    assert_eq!(element.id, "e1");
    s.click(&element).await.unwrap();
    s.send_keys(&element, "1000").await.unwrap();
    assert_eq!(s.text(&element).await.unwrap(), "Pending");
    assert!(s.is_enabled(&element).await.unwrap());
    s.delete().await.unwrap();
    assert_eq!(s.id(), "s1");
}

#[tokio::test]
async fn no_such_element_is_a_typed_server_error() {
    let server = MockServer::start().await;
    let s = session(&server).await;
    Mock::given(method("POST"))
        .and(path("/session/s1/element"))
        .respond_with(ResponseTemplate::new(404).set_body_json(json!({
            "value": {"error": "no such element", "message": "not found", "stacktrace": ""}
        })))
        .mount(&server)
        .await;

    let err = s
        .find_element(Locator::AccessibilityId, "missing")
        .await
        .unwrap_err();

    assert!(err.is_no_such_element());
    assert_eq!(err.to_string(), "webdriver no such element: not found");
}

#[tokio::test]
async fn source_screenshot_execute_and_timeouts() {
    let server = MockServer::start().await;
    let s = session(&server).await;
    Mock::given(method("GET"))
        .and(path("/session/s1/source"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"value": "<hierarchy/>"})))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/session/s1/screenshot"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"value": "iVBORw0KGgo="})))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/session/s1/execute/sync"))
        .and(body_partial_json(
            json!({"script": "return document.title"}),
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"value": {"ok": true}})))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/session/s1/timeouts"))
        .and(body_partial_json(json!({"implicit": 5000})))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"value": null})))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/session/s1/elements"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "value": [{ELEMENT_KEY: "a"}, {"ELEMENT": "b"}]
        })))
        .mount(&server)
        .await;

    assert_eq!(s.source().await.unwrap(), "<hierarchy/>");
    assert_eq!(s.screenshot_png().await.unwrap(), b"\x89PNG\r\n\x1a\n");
    let out = s
        .execute("return document.title", vec![json!({"page": "x"})])
        .await
        .unwrap();
    assert_eq!(out["ok"], true);
    s.set_implicit_wait(Duration::from_secs(5)).await.unwrap();
    let all = s.find_elements(Locator::XPath, "//*").await.unwrap();
    assert_eq!(all.len(), 2);
    assert_eq!(all[1].id, "b");
}

#[tokio::test]
async fn malformed_responses_and_status_endpoint() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/status"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"value": {"ready": true}})))
        .mount(&server)
        .await;
    assert!(server_ready(&server.uri(), T).await);
    assert!(!server_ready("http://127.0.0.1:1", Duration::from_millis(200)).await);

    let s = Session::attach(&server.uri(), "zz", T).unwrap();
    Mock::given(method("GET"))
        .and(path("/session/zz/source"))
        .respond_with(ResponseTemplate::new(200).set_body_string("not json"))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/session/zz/screenshot"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"value": "%%%"})))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/session/zz/element/e/text"))
        .respond_with(ResponseTemplate::new(500).set_body_json(json!({"value": {"unexpected": 1}})))
        .mount(&server)
        .await;

    assert!(matches!(
        s.source().await.unwrap_err(),
        WebDriverError::Shape(_)
    ));
    assert!(matches!(
        s.screenshot_png().await.unwrap_err(),
        WebDriverError::Shape(_)
    ));
    let err = s.text(&Element { id: "e".into() }).await.unwrap_err();
    assert!(matches!(err, WebDriverError::Server { .. }));
    assert_eq!(Locator::XPath.using(), "xpath");
    let decoded: serde_json::Map<String, Value> = decode(json!({"a": 1})).unwrap();
    assert_eq!(decoded["a"], 1);
}
