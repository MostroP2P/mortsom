#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

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

/// A settings page is taller than the phone, and Flutter does not build
/// what is not visible: a control below the fold is absent from the
/// accessibility tree until something scrolls to it. `find` already
/// knows that. A wait that does not would fail on a screen that is
/// behaving perfectly, and blame the app's automation contract for it.
#[tokio::test]
async fn a_wait_finds_a_control_below_the_fold_without_burning_its_deadline() {
    let server = MockServer::start().await;
    let s = session(&server).await;
    mount_below_the_fold(&server, "settings.mostro_node").await;

    let started = tokio::time::Instant::now();
    s.wait_for("settings.mostro_node", Duration::from_secs(30))
        .await
        .expect("a control below the fold is present, not missing");

    // Waiting out the whole deadline first would be correct and useless:
    // every off-screen control in a run would cost its full timeout.
    assert!(
        started.elapsed() < Duration::from_secs(10),
        "the below-the-fold probe must not wait for the deadline: {:?}",
        started.elapsed()
    );
}

/// The counterpart guarantee: a probe answers about the screen without
/// changing it. `go_home` asks "is the order book up yet?" on every
/// pass of its unwind loop, and `dismiss_notice` asks three such
/// questions per poll. A probe that scrolled would swipe a screen the
/// caller only meant to look at -- and leave it there.
#[tokio::test]
async fn a_probe_never_scrolls_the_screen_it_is_asked_about() {
    let server = MockServer::start().await;
    let s = session(&server).await;
    mount_below_the_fold(&server, "order.add.fab").await;

    assert!(!s.exists("order.add.fab").await.unwrap());
    assert!(
        !server
            .received_requests()
            .await
            .unwrap_or_default()
            .iter()
            .any(|r| String::from_utf8_lossy(&r.body).contains("UiScrollable")),
        "a probe must not swipe the screen to answer"
    );
}

/// The scrolling probe must not turn a genuinely missing control into a
/// hang or a false pass: it is searched for, not found, and the wait
/// still ends in a timeout.
#[tokio::test]
async fn a_control_that_is_nowhere_still_times_out() {
    let server = MockServer::start().await;
    let s = session(&server).await;
    Mock::given(method("POST"))
        .and(path("/session/s1/element"))
        .respond_with(
            ResponseTemplate::new(404)
                .set_body_json(json!({"value": {"error": "no such element", "message": ""}})),
        )
        .mount(&server)
        .await;

    let err = s
        .wait_for("nowhere", Duration::from_millis(300))
        .await
        .unwrap_err();

    assert!(matches!(
        err,
        AppiumError::WaitTimeout {
            condition: "not found",
            ..
        }
    ));
    assert!(!s.exists("nowhere").await.unwrap());
}

/// A scrolling search is not free: up to ten swipes, each a tree dump,
/// bounded by nothing but the HTTP timeout. A wait that starts one
/// after its deadline has passed can outlive the bound it advertised by
/// a minute -- and the callers that pass a short deadline are the ones
/// that pass it because they need the answer soon.
#[tokio::test]
async fn a_wait_never_outlives_its_deadline_by_more_than_the_deadline() {
    let server = MockServer::start().await;
    let s = session(&server).await;
    mount_slow_scroll(&server, Duration::from_secs(20)).await;

    let started = tokio::time::Instant::now();
    let err = s
        .wait_for("nowhere", Duration::from_millis(200))
        .await
        .unwrap_err();

    assert!(matches!(err, AppiumError::WaitTimeout { .. }), "{err:?}");
    // The probe may cost the wait at most what the wait itself is worth.
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "a 200ms wait must not hang on an unbounded probe: {:?}",
        started.elapsed()
    );
}

/// A dead session, a broken transport or a malformed response is an
/// infrastructure failure. Reported as `WaitTimeout` it reads as a
/// missing automation control instead, and the run blames the app for
/// the harness.
#[tokio::test]
async fn a_driver_failure_during_the_probe_is_not_a_missing_control() {
    let server = MockServer::start().await;
    let s = session(&server).await;
    mount_scroll_failure(&server).await;

    let err = s
        .wait_for("settings.mostro_node", Duration::from_millis(200))
        .await
        .unwrap_err();

    assert!(
        matches!(
            &err,
            AppiumError::WebDriver(WebDriverError::Server { error, .. })
                if error == "invalid session id"
        ),
        "the driver's own failure must surface as itself: {err:?}"
    );
}

/// The same guarantee for the plain lookup: `find` falls back to a
/// scrolling search, and if that search fails for a reason of its own
/// the caller hears about that reason, not about the element being
/// off-screen.
#[tokio::test]
async fn find_surfaces_a_driver_failure_from_its_scrolling_fallback() {
    let server = MockServer::start().await;
    let s = session(&server).await;
    mount_scroll_failure(&server).await;

    let err = s.find("settings.mostro_node").await.unwrap_err();

    assert!(
        matches!(
            &err,
            AppiumError::WebDriver(WebDriverError::Server { error, .. })
                if error == "invalid session id"
        ),
        "the driver's own failure must surface as itself: {err:?}"
    );
}

/// `find` falls back to a scrolling search, and that search is ten
/// swipes over a device that may be busy. Left to the session's HTTP
/// timeout, one lookup for a control that is simply not there costs a
/// minute -- and a scenario looks up controls constantly, so the cost
/// is per call, not per run.
#[tokio::test]
async fn a_lookup_bounds_the_scrolling_search_it_falls_back_to() {
    let server = MockServer::start().await;
    let s = session(&server).await;
    mount_slow_scroll(&server, Duration::from_secs(20)).await;

    let started = tokio::time::Instant::now();
    let err = s
        .find_within("nowhere", Duration::from_millis(200))
        .await
        .unwrap_err();

    // A search cut short never answered the question. Reported as the
    // plain lookup's `no such element` it answers it anyway, and says
    // the app never rendered a control the search never got to look for.
    assert!(
        matches!(&err, AppiumError::ProbeUnfinished { accessibility_id, .. }
            if accessibility_id == "nowhere"),
        "out of budget is not evidence of absence: {err:?}"
    );
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "the scrolling fallback must be bounded: {:?}",
        started.elapsed()
    );
}

/// The counterpart: a search that did finish and found nothing is an
/// answer, and the answer is that the control is not there.
#[tokio::test]
async fn a_lookup_that_searches_to_the_end_reports_the_control_missing() {
    let server = MockServer::start().await;
    let s = session(&server).await;
    Mock::given(method("POST"))
        .and(path("/session/s1/element"))
        .respond_with(
            ResponseTemplate::new(404)
                .set_body_json(json!({"value": {"error": "no such element", "message": ""}})),
        )
        .mount(&server)
        .await;

    let err = s.find("nowhere").await.unwrap_err();

    assert!(
        matches!(&err, AppiumError::WebDriver(e) if e.is_no_such_element()),
        "{err:?}"
    );
}

/// A wait spends its probes out of its own deadline, and a short one
/// routinely cuts them off: for a wait, out of budget is not a failure
/// at all, it is a reason to keep polling. The typed error belongs to
/// the lookup that has no deadline to keep polling against.
#[tokio::test]
async fn a_wait_keeps_polling_through_a_probe_it_cut_short() {
    let server = MockServer::start().await;
    let s = session(&server).await;
    mount_slow_scroll(&server, Duration::from_secs(20)).await;

    let err = s
        .wait_for("nowhere", Duration::from_millis(200))
        .await
        .unwrap_err();

    assert!(
        matches!(err, AppiumError::WaitTimeout { .. }),
        "a cut-short probe must not end the wait as a driver failure: {err:?}"
    );
}

/// Nothing is on screen and every scrolling search stalls for `delay`.
async fn mount_slow_scroll(server: &MockServer, delay: Duration) {
    Mock::given(method("POST"))
        .and(path("/session/s1/element"))
        .and(body_string_contains("UiScrollable"))
        .respond_with(
            ResponseTemplate::new(404)
                .set_delay(delay)
                .set_body_json(json!({"value": {"error": "no such element", "message": ""}})),
        )
        .mount(server)
        .await;
    Mock::given(method("POST"))
        .and(path("/session/s1/element"))
        .respond_with(
            ResponseTemplate::new(404)
                .set_body_json(json!({"value": {"error": "no such element", "message": ""}})),
        )
        .mount(server)
        .await;
}

/// Nothing is on screen and the scrolling search fails for a reason
/// that has nothing to do with the element: the session is gone.
async fn mount_scroll_failure(server: &MockServer) {
    Mock::given(method("POST"))
        .and(path("/session/s1/element"))
        .and(body_string_contains("UiScrollable"))
        .respond_with(ResponseTemplate::new(404).set_body_json(
            json!({"value": {"error": "invalid session id", "message": "session deleted"}}),
        ))
        .mount(server)
        .await;
    Mock::given(method("POST"))
        .and(path("/session/s1/element"))
        .respond_with(
            ResponseTemplate::new(404)
                .set_body_json(json!({"value": {"error": "no such element", "message": ""}})),
        )
        .mount(server)
        .await;
}

/// A control that is off-screen: the plain lookup misses it until
/// something scrolls to it, and then finds it where it now is.
async fn mount_below_the_fold(server: &MockServer, id: &str) {
    let scrolled = Arc::new(AtomicBool::new(false));
    let flag = scrolled.clone();
    let scroll_for_id = format!("scrollIntoView(new UiSelector().resourceId(\\\"{id}\\\")");
    Mock::given(method("POST"))
        .and(path("/session/s1/element"))
        .and(move |request: &wiremock::Request| {
            let hit = String::from_utf8_lossy(&request.body).contains(scroll_for_id.as_str());
            if hit {
                flag.store(true, Ordering::SeqCst);
            }
            hit
        })
        // As the real driver answers: the container, not the target.
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({"value": {ELEMENT_KEY: "the-list"}})),
        )
        .mount(server)
        .await;
    let plain = format!("\"new UiSelector().resourceId(\\\"{id}\\\")\"");
    Mock::given(method("POST"))
        .and(path("/session/s1/element"))
        .and(move |request: &wiremock::Request| {
            String::from_utf8_lossy(&request.body).contains(plain.as_str())
                && scrolled.load(Ordering::SeqCst)
        })
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({"value": {ELEMENT_KEY: "e1"}})),
        )
        .mount(server)
        .await;
    mount_nothing_else(server).await;
}

/// Every scrolling call answers with the scrollable container, exactly
/// as UiAutomator2 does.
async fn mount_scroll_answering_with_the_list(server: &MockServer) {
    Mock::given(method("POST"))
        .and(path("/session/s1/element"))
        .and(|request: &wiremock::Request| {
            String::from_utf8_lossy(&request.body).contains("UiScrollable")
        })
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({"value": {ELEMENT_KEY: "the-list"}})),
        )
        .mount(server)
        .await;
}

async fn mount_nothing_else(server: &MockServer) {
    Mock::given(method("POST"))
        .and(path("/session/s1/element"))
        .respond_with(
            ResponseTemplate::new(404)
                .set_body_json(json!({"value": {"error": "no such element", "message": ""}})),
        )
        .mount(server)
        .await;
}

/// UiAutomator's `scrollIntoView` evaluates to the `UiScrollable` it
/// was called on, so Appium answers `200` with the scrollable container
/// whether or not the search found anything. A control the app never
/// rendered is reported as present, and clicking the answer taps the
/// middle of the list.
#[tokio::test]
async fn a_scroll_that_found_nothing_is_not_a_control() {
    let server = MockServer::start().await;
    let s = session(&server).await;
    mount_scroll_answering_with_the_list(&server).await;
    mount_nothing_else(&server).await;

    assert!(s.find("order.confirm.home").await.is_err());
    assert!(matches!(
        s.wait_for("order.confirm.home", Duration::from_millis(300))
            .await
            .unwrap_err(),
        AppiumError::WaitTimeout { .. }
    ));
    assert!(s.tap("order.confirm.home").await.is_err());

    // The list is not the button. Tapping it presses whatever sits in
    // the middle of the screen.
    assert!(
        !server
            .received_requests()
            .await
            .unwrap_or_default()
            .iter()
            .any(|r| r.url.path().ends_with("/click")),
        "nothing may be clicked for a control that is not there"
    );
}

/// The scrolling call is only half of the search: the plain lookup
/// that follows it is the half that decides. A driver failure there is
/// still the driver's, and reporting it as `no such element` would say
/// the app never rendered a control nobody ever managed to look for.
#[tokio::test]
async fn a_failure_in_the_deciding_lookup_is_not_a_missing_control() {
    let server = MockServer::start().await;
    let s = session(&server).await;
    let scrolled = Arc::new(AtomicBool::new(false));
    let flag = scrolled.clone();
    Mock::given(method("POST"))
        .and(path("/session/s1/element"))
        .and(move |request: &wiremock::Request| {
            let hit = String::from_utf8_lossy(&request.body).contains("UiScrollable");
            if hit {
                flag.store(true, Ordering::SeqCst);
            }
            hit
        })
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({"value": {ELEMENT_KEY: "the-list"}})),
        )
        .mount(&server)
        .await;
    // The scroll works; only the lookup that reads its result fails, so
    // the failure can come from nowhere but the deciding half.
    Mock::given(method("POST"))
        .and(path("/session/s1/element"))
        .and(move |_: &wiremock::Request| scrolled.load(Ordering::SeqCst))
        .respond_with(ResponseTemplate::new(404).set_body_json(
            json!({"value": {"error": "invalid session id", "message": "session deleted"}}),
        ))
        .mount(&server)
        .await;
    mount_nothing_else(&server).await;

    for err in [
        s.find("settings.mostro_node").await.unwrap_err(),
        s.wait_for("settings.mostro_node", Duration::from_millis(200))
            .await
            .unwrap_err(),
    ] {
        assert!(
            matches!(
                &err,
                AppiumError::WebDriver(WebDriverError::Server { error, .. })
                    if error == "invalid session id"
            ),
            "the driver's own failure must surface as itself: {err:?}"
        );
    }
}

/// A request that never got an answer is not the element being absent and
/// it is not the wait's verdict either: the poll keeps going until the
/// deadline. A blip while the app is being reactivated used to end the wait
/// -- and the run -- as an automation-contract failure.
#[tokio::test]
async fn a_wait_polls_through_a_transient_transport_failure() {
    let server = MockServer::start().await;
    let raw = Session::attach(&server.uri(), "s1", Duration::from_millis(100)).unwrap();
    let s = AppiumSession::from_session(raw, "network.mostro.app");
    Mock::given(method("POST"))
        .and(path("/session/s1/element"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"value": {ELEMENT_KEY: "e1"}}))
                .set_delay(Duration::from_millis(400)),
        )
        .up_to_n_times(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/session/s1/element"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({"value": {ELEMENT_KEY: "e1"}})),
        )
        .mount(&server)
        .await;

    s.wait_for("appbar.drawer", Duration::from_secs(5))
        .await
        .expect("a transport blip mid-wait is not an answer about the app");
}

/// When every poll went unanswered, the wait must not claim the element was
/// missing: nothing ever looked at the screen.
#[tokio::test]
async fn a_wait_that_never_got_an_answer_reports_the_transport_failure() {
    let server = MockServer::start().await;
    let raw = Session::attach(&server.uri(), "s1", Duration::from_millis(100)).unwrap();
    let s = AppiumSession::from_session(raw, "network.mostro.app");
    Mock::given(method("POST"))
        .and(path("/session/s1/element"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"value": {ELEMENT_KEY: "e1"}}))
                .set_delay(Duration::from_millis(400)),
        )
        .mount(&server)
        .await;

    let err = s
        .wait_for("appbar.drawer", Duration::from_millis(300))
        .await
        .unwrap_err();
    assert!(
        matches!(&err, AppiumError::WebDriver(e) if e.is_transport()),
        "an unanswered wait says so, it does not report the element absent: {err:?}"
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
