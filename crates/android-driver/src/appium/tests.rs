#![allow(clippy::unwrap_used, clippy::expect_used)]

use super::*;
use crate::process::tests_support::FakeRunner;
use wiremock::matchers::{body_partial_json, body_string_contains, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const ELEMENT_KEY: &str = "element-6066-11e4-a52e-4f735466cecf";

fn spec() -> SessionSpec {
    SessionSpec {
        udid: "emulator-5554".into(),
        system_port: 8200,
        app_package: "network.mostro.app".into(),
        app_activity: None,
    }
}

#[test]
fn a_scrolling_lookup_searches_for_the_same_id() {
    let selector = scroll_into_view_selector("settings.mostro_node");
    assert!(selector.contains("UiScrollable"), "{selector}");
    assert!(selector.contains("scrollable(true)"), "{selector}");
    assert!(
        selector.contains("resourceId(\"settings.mostro_node\")"),
        "{selector}"
    );
    assert!(
        scroll_into_view_selector("a\"b").contains("a\\\"b"),
        "quotes are escaped in the scrolling selector too"
    );
}

#[test]
fn resource_id_selector_escapes_quotes() {
    assert_eq!(
        resource_id_selector("order.status"),
        "new UiSelector().resourceId(\"order.status\")"
    );
    assert_eq!(
        resource_id_selector("a\"b"),
        "new UiSelector().resourceId(\"a\\\"b\")"
    );
}

#[test]
fn capabilities_pin_uiautomator2_and_device() {
    let caps = spec().capabilities();
    assert_eq!(caps["platformName"], "Android");
    assert_eq!(caps["appium:automationName"], "UiAutomator2");
    assert_eq!(caps["appium:udid"], "emulator-5554");
    assert_eq!(caps["appium:systemPort"], 8200);
    assert_eq!(caps["appium:noReset"], true);
    assert_eq!(caps["appium:autoLaunch"], false);
    assert_eq!(caps["appium:adbExecTimeout"], ADB_EXEC_TIMEOUT_MS);
    assert!(caps.get("appium:appActivity").is_none());
    let mut with_activity = spec();
    with_activity.app_activity = Some(".MainActivity".into());
    assert_eq!(
        with_activity.capabilities()["appium:appActivity"],
        ".MainActivity"
    );
}

async fn session(server: &MockServer) -> AppiumSession {
    Mock::given(method("POST"))
        .and(path("/session"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({"value": {"sessionId": "s1"}})),
        )
        .mount(server)
        .await;
    AppiumSession::create(&server.uri(), &spec()).await.unwrap()
}

/// A field that never took focus ignores set-text and answers success
/// anyway, so the form fills with nothing and the app is blamed for
/// refusing it.
#[tokio::test]
async fn typing_focuses_the_field_before_it_types() {
    let server = MockServer::start().await;
    let s = session(&server).await;
    Mock::given(method("POST"))
        .and(path("/session/s1/element"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({"value": {ELEMENT_KEY: "e1"}})),
        )
        .mount(&server)
        .await;
    for endpoint in ["click", "clear", "value"] {
        Mock::given(method("POST"))
            .and(path(format!("/session/s1/element/e1/{endpoint}")))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"value": null})))
            .mount(&server)
            .await;
    }

    s.type_text("order.create.sats_amount", "1000")
        .await
        .unwrap();

    let calls: Vec<String> = server
        .received_requests()
        .await
        .unwrap_or_default()
        .iter()
        .map(|r| r.url.path().to_owned())
        .filter(|p| p.starts_with("/session/s1/element/e1/"))
        .collect();
    assert_eq!(
        calls,
        vec![
            "/session/s1/element/e1/click",
            "/session/s1/element/e1/clear",
            "/session/s1/element/e1/value",
        ],
        "focus, then clear, then type"
    );
}

#[tokio::test]
async fn primitives_map_to_webdriver_calls() {
    let server = MockServer::start().await;
    let s = session(&server).await;
    Mock::given(method("POST"))
        .and(path("/session/s1/element"))
        .and(body_string_contains("-android uiautomator"))
        .and(body_string_contains("resourceId(\\\"order.status\\\")"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({"value": {ELEMENT_KEY: "e1"}})),
        )
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/session/s1/element"))
        .and(body_string_contains("resourceId(\\\"missing\\\")"))
        .respond_with(
            ResponseTemplate::new(404)
                .set_body_json(json!({"value": {"error": "no such element", "message": ""}})),
        )
        .mount(&server)
        .await;
    for endpoint in ["click", "clear", "value"] {
        Mock::given(method("POST"))
            .and(path(format!("/session/s1/element/e1/{endpoint}")))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"value": null})))
            .mount(&server)
            .await;
    }
    Mock::given(method("GET"))
        .and(path("/session/s1/element/e1/text"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"value": "Pending"})))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/session/s1/element/e1/attribute/content-desc"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"value": "pending"})))
        .mount(&server)
        .await;
    for (script, args) in [
        (
            "mobile: setConnectivity",
            json!({"wifi": false, "data": false}),
        ),
        ("mobile: backgroundApp", json!({"seconds": 1})),
        (
            "mobile: terminateApp",
            json!({"appId": "network.mostro.app"}),
        ),
        (
            "mobile: activateApp",
            json!({"appId": "network.mostro.app"}),
        ),
        ("mobile: clearApp", json!({"appId": "network.mostro.app"})),
    ] {
        Mock::given(method("POST"))
            .and(path("/session/s1/execute/sync"))
            .and(body_partial_json(json!({"script": script, "args": [args]})))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"value": null})))
            .expect(1)
            .mount(&server)
            .await;
    }
    Mock::given(method("DELETE"))
        .and(path("/session/s1"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"value": null})))
        .mount(&server)
        .await;

    s.tap("order.status").await.unwrap();
    s.type_text("order.status", "1000").await.unwrap();
    assert_eq!(s.text_of("order.status").await.unwrap(), "Pending");
    assert_eq!(s.label_of("order.status").await.unwrap(), "pending");
    assert!(s.exists("order.status").await.unwrap());
    assert!(!s.exists("missing").await.unwrap());
    s.wait_for("order.status", Duration::from_secs(1))
        .await
        .unwrap();
    s.wait_gone("missing", Duration::from_secs(1))
        .await
        .unwrap();
    Mock::given(method("POST"))
        .and(path("/session/s1/orientation"))
        .and(body_partial_json(json!({"orientation": "LANDSCAPE"})))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"value": null})))
        .expect(1)
        .mount(&server)
        .await;
    s.set_connectivity(false).await.unwrap();
    s.set_orientation(true).await.unwrap();
    s.background(1).await.unwrap();
    assert!(matches!(
        s.background(MAX_BACKGROUND_SECS + 1).await.unwrap_err(),
        AppiumError::BackgroundTooLong(_)
    ));
    s.relaunch().await.unwrap();
    s.clear_app().await.unwrap();
    // Clearing revokes what the install granted; the harness grants it
    // back or the next launch opens on a permission dialog.
    assert!(
        server
            .received_requests()
            .await
            .unwrap_or_default()
            .iter()
            .any(|r| String::from_utf8_lossy(&r.body).contains("changePermissions")),
        "clearing the app must re-grant its runtime permissions"
    );
    s.end().await.unwrap();

    let err = s
        .wait_for("missing", Duration::from_millis(300))
        .await
        .unwrap_err();
    assert!(matches!(
        err,
        AppiumError::WaitTimeout {
            condition: "not found",
            ..
        }
    ));
    let err = s
        .wait_gone("order.status", Duration::from_millis(300))
        .await
        .unwrap_err();
    assert!(matches!(
        err,
        AppiumError::WaitTimeout {
            condition: "still present",
            ..
        }
    ));
    assert_eq!(s.raw().id(), "s1");
}

#[tokio::test]
async fn server_start_fails_when_status_never_ready() {
    let dir = tempfile::tempdir().unwrap();
    let runner = FakeRunner::default();

    let err = AppiumServer::start(
        &runner,
        Path::new("/bin/appium"),
        1,
        dir.path(),
        Duration::from_millis(600),
    )
    .await
    .unwrap_err();

    assert!(matches!(err, AppiumError::NotReady { .. }));
    assert!(runner.spawned()[0].contains("--port 1"));
}

#[tokio::test]
async fn external_server_reports_url_and_stops() {
    let runner = FakeRunner::default();
    let child = runner.spawn(&Command::new("x", &[]), None).await.unwrap();
    let server = AppiumServer::external(4723, child);
    assert_eq!(server.base_url(), "http://127.0.0.1:4723");
    server.stop().await.unwrap();
}
/// Selectable text (an identity, an invoice) carries no accessibility
/// description, and reading only that reports the screen as empty.
#[tokio::test]
async fn a_label_falls_back_to_the_element_text() {
    let server = MockServer::start().await;
    let s = session(&server).await;
    Mock::given(method("POST"))
        .and(path("/session/s1/element"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({"value": {ELEMENT_KEY: "e1"}})),
        )
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/session/s1/element/e1/attribute/content-desc"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"value": ""})))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/session/s1/element/e1/attribute/text"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"value": "npub1alice"})))
        .mount(&server)
        .await;

    assert_eq!(s.label_of("keys.public_key").await.unwrap(), "npub1alice");
}
