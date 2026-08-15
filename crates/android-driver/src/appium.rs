//! Appium server lifecycle and UiAutomator2 session primitives (spec Phase
//! 2.4): find by accessibility id, tap, type, wait, screenshot, hierarchy
//! dump, background, force-stop, relaunch and clear-data. Every wait is a
//! bounded predicate; there are no fixed sleeps in the primitives.

use std::path::Path;
use std::time::Duration;

use serde_json::{Value, json};
use thiserror::Error;
use tracing::info;

use crate::process::{ChildProcess, Command, CommandRunner, ProcessError};
use crate::webdriver::{Element, Locator, Session, WebDriverError, server_ready};

/// Interval between element polls inside a bounded wait.
pub const POLL_INTERVAL: Duration = Duration::from_millis(250);
/// Interval between server readiness probes.
pub const SERVER_POLL: Duration = Duration::from_millis(500);
/// Appium's own idle timeout for a session, in seconds.
pub const NEW_COMMAND_TIMEOUT_SECS: u64 = 300;
/// HTTP timeout for one WebDriver call.
pub const HTTP_TIMEOUT: Duration = Duration::from_secs(60);
/// Longest background interval a primitive accepts.
pub const MAX_BACKGROUND_SECS: u64 = 300;

/// Appium failures.
#[derive(Debug, Error)]
pub enum AppiumError {
    /// The server process could not be started or died.
    #[error(transparent)]
    Process(#[from] ProcessError),
    /// The server did not answer `/status` in time.
    #[error("appium server at {url} not ready after {timeout:?}")]
    NotReady {
        /// Base URL probed.
        url: String,
        /// Deadline used.
        timeout: Duration,
    },
    /// A WebDriver call failed.
    #[error(transparent)]
    WebDriver(#[from] WebDriverError),
    /// A background interval exceeded [`MAX_BACKGROUND_SECS`].
    #[error("background of {0}s exceeds the maximum of {MAX_BACKGROUND_SECS}s")]
    BackgroundTooLong(u64),
    /// An element did not appear (or disappear) before the deadline.
    #[error("element `{accessibility_id}` {condition} within {timeout:?}")]
    WaitTimeout {
        /// Accessibility id waited on.
        accessibility_id: String,
        /// `not found` or `still present`.
        condition: &'static str,
        /// Deadline used.
        timeout: Duration,
    },
}

/// A running Appium server owned by this run.
pub struct AppiumServer {
    port: u16,
    process: Box<dyn ChildProcess>,
}

impl std::fmt::Debug for AppiumServer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AppiumServer")
            .field("port", &self.port)
            .finish_non_exhaustive()
    }
}

impl AppiumServer {
    /// Starts `appium` on `port` (loopback only) and waits for `/status`.
    pub async fn start(
        runner: &dyn CommandRunner,
        appium: &Path,
        port: u16,
        log_dir: &Path,
        ready_timeout: Duration,
    ) -> Result<Self, AppiumError> {
        let port_text = port.to_string();
        let command = Command::new(
            appium,
            &[
                "--address",
                "127.0.0.1",
                "--port",
                &port_text,
                "--log-level",
                "info",
                "--relaxed-security",
            ],
        );
        let log = log_dir.join("appium.log");
        let process = runner.spawn(&command, Some(&log)).await?;
        let server = Self { port, process };
        let deadline = tokio::time::Instant::now() + ready_timeout;
        loop {
            if server_ready(&server.base_url(), SERVER_POLL).await {
                info!(port, "appium server ready");
                return Ok(server);
            }
            if !server.process.is_alive().await || tokio::time::Instant::now() >= deadline {
                let _ = server.process.kill().await;
                return Err(AppiumError::NotReady {
                    url: server.base_url(),
                    timeout: ready_timeout,
                });
            }
            tokio::time::sleep(SERVER_POLL).await;
        }
    }

    /// Wraps an already running server (tests, external Appium).
    pub fn external(port: u16, process: Box<dyn ChildProcess>) -> Self {
        Self { port, process }
    }

    /// Base URL of the server.
    pub fn base_url(&self) -> String {
        format!("http://127.0.0.1:{}", self.port)
    }

    /// Stops the server.
    pub async fn stop(&self) -> Result<(), AppiumError> {
        Ok(self.process.kill().await?)
    }
}

/// Session parameters for one device. `system_port` and the server port
/// must be unique per device; the coordinator allocates them per actor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionSpec {
    /// adb serial (`emulator-5554`).
    pub udid: String,
    /// Distinct UiAutomator2 server port per device (`appium:systemPort`).
    pub system_port: u16,
    /// Application package.
    pub app_package: String,
    /// Launch activity; `None` lets UiAutomator2 resolve the launcher.
    pub app_activity: Option<String>,
}

impl SessionSpec {
    /// W3C `alwaysMatch` capabilities. `noReset` keeps app data untouched at
    /// session start and end: Appium never reinstalls or wipes the app, so
    /// clean state is Mortsom's responsibility. Callers must `clear_app` (or
    /// restore the emulator snapshot) before the first use in a run.
    pub fn capabilities(&self) -> Value {
        let mut caps = json!({
            "platformName": "Android",
            "appium:automationName": "UiAutomator2",
            "appium:udid": self.udid,
            "appium:systemPort": self.system_port,
            "appium:appPackage": self.app_package,
            "appium:noReset": true,
            "appium:autoGrantPermissions": true,
            "appium:disableWindowAnimation": true,
            "appium:newCommandTimeout": NEW_COMMAND_TIMEOUT_SECS,
            "appium:ignoreHiddenApiPolicyError": true,
        });
        if let Some(activity) = &self.app_activity {
            caps["appium:appActivity"] = Value::String(activity.clone());
        }
        caps
    }
}

/// UiAutomator2 session with domain-neutral primitives.
#[derive(Debug, Clone)]
pub struct AppiumSession {
    session: Session,
    package: String,
}

impl AppiumSession {
    /// Creates a session on `server` for `spec`.
    pub async fn create(server_url: &str, spec: &SessionSpec) -> Result<Self, AppiumError> {
        let session = Session::create(server_url, spec.capabilities(), HTTP_TIMEOUT).await?;
        Ok(Self {
            session,
            package: spec.app_package.clone(),
        })
    }

    /// Wraps an existing session.
    pub fn from_session(session: Session, package: impl Into<String>) -> Self {
        Self {
            session,
            package: package.into(),
        }
    }

    /// The raw WebDriver session.
    pub fn raw(&self) -> &Session {
        &self.session
    }

    /// Finds an element by accessibility id, failing immediately if absent.
    pub async fn find(&self, accessibility_id: &str) -> Result<Element, AppiumError> {
        Ok(self
            .session
            .find_element(Locator::AccessibilityId, accessibility_id)
            .await?)
    }

    /// Whether an element with this accessibility id is present right now.
    pub async fn exists(&self, accessibility_id: &str) -> Result<bool, AppiumError> {
        match self
            .session
            .find_element(Locator::AccessibilityId, accessibility_id)
            .await
        {
            Ok(_) => Ok(true),
            Err(e) if e.is_no_such_element() => Ok(false),
            Err(e) => Err(e.into()),
        }
    }

    /// Waits until the element is present, bounded by `timeout`.
    pub async fn wait_for(
        &self,
        accessibility_id: &str,
        timeout: Duration,
    ) -> Result<Element, AppiumError> {
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            match self
                .session
                .find_element(Locator::AccessibilityId, accessibility_id)
                .await
            {
                Ok(element) => return Ok(element),
                Err(e) if e.is_no_such_element() => {}
                Err(e) => return Err(e.into()),
            }
            if tokio::time::Instant::now() >= deadline {
                return Err(AppiumError::WaitTimeout {
                    accessibility_id: accessibility_id.to_owned(),
                    condition: "not found",
                    timeout,
                });
            }
            tokio::time::sleep(POLL_INTERVAL).await;
        }
    }

    /// Waits until no element with this id is present (negative observation).
    pub async fn wait_gone(
        &self,
        accessibility_id: &str,
        timeout: Duration,
    ) -> Result<(), AppiumError> {
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            if !self.exists(accessibility_id).await? {
                return Ok(());
            }
            if tokio::time::Instant::now() >= deadline {
                return Err(AppiumError::WaitTimeout {
                    accessibility_id: accessibility_id.to_owned(),
                    condition: "still present",
                    timeout,
                });
            }
            tokio::time::sleep(POLL_INTERVAL).await;
        }
    }

    /// Taps an element found by accessibility id.
    pub async fn tap(&self, accessibility_id: &str) -> Result<(), AppiumError> {
        let element = self.find(accessibility_id).await?;
        Ok(self.session.click(&element).await?)
    }

    /// Clears then types into an element found by accessibility id. The text
    /// may be a secret (NWC URI, mnemonic) and is never logged.
    #[tracing::instrument(skip(self, text), fields(id = accessibility_id))]
    pub async fn type_text(&self, accessibility_id: &str, text: &str) -> Result<(), AppiumError> {
        let element = self.find(accessibility_id).await?;
        self.session.clear(&element).await?;
        Ok(self.session.send_keys(&element, text).await?)
    }

    /// Visible text of an element.
    pub async fn text_of(&self, accessibility_id: &str) -> Result<String, AppiumError> {
        let element = self.find(accessibility_id).await?;
        Ok(self.session.text(&element).await?)
    }

    /// Whether an element is enabled.
    pub async fn is_enabled(&self, accessibility_id: &str) -> Result<bool, AppiumError> {
        let element = self.find(accessibility_id).await?;
        Ok(self.session.is_enabled(&element).await?)
    }

    /// PNG screenshot.
    pub async fn screenshot_png(&self) -> Result<Vec<u8>, AppiumError> {
        Ok(self.session.screenshot_png().await?)
    }

    /// Accessibility hierarchy as XML.
    pub async fn page_source(&self) -> Result<String, AppiumError> {
        Ok(self.session.source().await?)
    }

    /// Sends the app to the background for `secs` seconds and returns to it.
    pub async fn background(&self, secs: u64) -> Result<(), AppiumError> {
        if secs > MAX_BACKGROUND_SECS {
            return Err(AppiumError::BackgroundTooLong(secs));
        }
        self.session
            .execute("mobile: backgroundApp", vec![json!({ "seconds": secs })])
            .await?;
        Ok(())
    }

    /// Force-stops the app (`mobile: terminateApp`).
    pub async fn terminate_app(&self) -> Result<(), AppiumError> {
        self.session
            .execute(
                "mobile: terminateApp",
                vec![json!({ "appId": self.package })],
            )
            .await?;
        Ok(())
    }

    /// Brings the app to the foreground, launching it if needed.
    pub async fn activate_app(&self) -> Result<(), AppiumError> {
        self.session
            .execute(
                "mobile: activateApp",
                vec![json!({ "appId": self.package })],
            )
            .await?;
        Ok(())
    }

    /// Force-stop followed by activation.
    pub async fn relaunch(&self) -> Result<(), AppiumError> {
        self.terminate_app().await?;
        self.activate_app().await
    }

    /// Clears the app's data (`mobile: clearApp`), leaving it stopped.
    pub async fn clear_app(&self) -> Result<(), AppiumError> {
        self.session
            .execute("mobile: clearApp", vec![json!({ "appId": self.package })])
            .await?;
        Ok(())
    }

    /// Ends the session.
    pub async fn end(&self) -> Result<(), AppiumError> {
        Ok(self.session.delete().await?)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::process::tests_support::FakeRunner;
    use wiremock::matchers::{body_partial_json, method, path};
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
    fn capabilities_pin_uiautomator2_and_device() {
        let caps = spec().capabilities();
        assert_eq!(caps["platformName"], "Android");
        assert_eq!(caps["appium:automationName"], "UiAutomator2");
        assert_eq!(caps["appium:udid"], "emulator-5554");
        assert_eq!(caps["appium:systemPort"], 8200);
        assert_eq!(caps["appium:noReset"], true);
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

    #[tokio::test]
    async fn primitives_map_to_webdriver_calls() {
        let server = MockServer::start().await;
        let s = session(&server).await;
        Mock::given(method("POST"))
            .and(path("/session/s1/element"))
            .and(body_partial_json(
                json!({"using": "accessibility id", "value": "order.status"}),
            ))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(json!({"value": {ELEMENT_KEY: "e1"}})),
            )
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/session/s1/element"))
            .and(body_partial_json(json!({"value": "missing"})))
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
        for (script, args) in [
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
        assert!(s.exists("order.status").await.unwrap());
        assert!(!s.exists("missing").await.unwrap());
        s.wait_for("order.status", Duration::from_secs(1))
            .await
            .unwrap();
        s.wait_gone("missing", Duration::from_secs(1))
            .await
            .unwrap();
        s.background(1).await.unwrap();
        assert!(matches!(
            s.background(MAX_BACKGROUND_SECS + 1).await.unwrap_err(),
            AppiumError::BackgroundTooLong(_)
        ));
        s.relaunch().await.unwrap();
        s.clear_app().await.unwrap();
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
}
