//! Appium server lifecycle and UiAutomator2 session primitives (spec Phase
//! 2.4): find by semantic identifier, tap, type, wait, screenshot, hierarchy
//! dump, background, force-stop, relaunch and clear-data. Every wait is a
//! bounded predicate; there are no fixed sleeps in the primitives.
//!
//! Semantic identifiers (`Semantics.identifier` in Flutter) surface on
//! Android as the accessibility `resource-id`, so lookups use a UiAutomator
//! `resourceId(...)` selector, which matches the raw string exactly.

use std::path::Path;
use std::time::Duration;

use serde_json::{Value, json};
use thiserror::Error;
use tracing::{info, warn};

use crate::process::{ChildProcess, Command, CommandRunner, ProcessError};
use crate::webdriver::{Element, Locator, Session, WebDriverError, server_ready};

/// How many swipes a scrolling search may take before giving up.
pub const SCROLL_SEARCH_SWIPES: u32 = 10;
/// Interval between element polls inside a bounded wait.
pub const POLL_INTERVAL: Duration = Duration::from_millis(250);
/// How long a bounded wait polls the plain lookup before it also looks
/// below the fold. Long enough that a screen still being built is not
/// swiped while it renders; short enough that an off-screen control costs
/// a moment rather than its whole deadline.
pub const SCROLL_PROBE_DELAY: Duration = Duration::from_secs(2);
/// Longest a single below-the-fold probe may run. A scrolling search is up
/// to [`SCROLL_SEARCH_SWIPES`] swipes, each one a tree dump, and left alone
/// it is bounded only by [`HTTP_TIMEOUT`]: a wait would advertise seconds
/// and cost a minute, and a plain lookup for a control that is not there
/// would cost one too. A wait spends at most this much on a probe, and
/// never more than the wait itself is worth; a lookup, which has no
/// deadline of its own, spends exactly this much.
pub const SCROLL_PROBE_BUDGET: Duration = Duration::from_secs(10);
/// Interval between server readiness probes.
pub const SERVER_POLL: Duration = Duration::from_millis(500);
/// Appium's own idle timeout for a session, in seconds.
/// Appium's idle-session watchdog, disabled on purpose.
///
/// It exists to reap sessions abandoned by a crashed client. Mortsom is not
/// one: it ends every session explicitly, kills its emulators on teardown,
/// and `mortsom cleanup` sweeps what an interrupted run left. Meanwhile an
/// actor legitimately waits minutes for its counterparty -- a trade that
/// expires unpaid waits far longer -- and a watchdog would kill the waiting
/// side mid-scenario, reporting it as a dead session rather than as the
/// wait it was.
pub const NEW_COMMAND_TIMEOUT_SECS: u64 = 0;
/// Deadline Appium gives adb commands (debug Flutter builds start slowly on
/// software-rendered emulators), in milliseconds.
pub const ADB_EXEC_TIMEOUT_MS: u64 = 120_000;
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
    /// A scrolling search did not finish inside the budget it was given.
    ///
    /// Distinct from the element being absent on purpose: this search never
    /// got to answer, so nothing here says anything about the app. It is
    /// the harness being slow -- a busy software-rendered emulator, an
    /// Appium that is not keeping up.
    #[error("scrolling search for `{accessibility_id}` did not finish within {budget:?}")]
    ProbeUnfinished {
        /// Accessibility id searched for.
        accessibility_id: String,
        /// Budget the search was given.
        budget: Duration,
    },
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

/// UiAutomator selector matching a Flutter semantic identifier exactly.
pub fn resource_id_selector(identifier: &str) -> String {
    let escaped = escape_selector(identifier);
    format!("new UiSelector().resourceId(\"{escaped}\")")
}

/// The same lookup, scrolling the first scrollable container until the
/// element comes into view. A phone screen shows a fraction of a settings
/// page, and Flutter does not build what is not visible: without this, a
/// control below the fold does not exist as far as the driver can tell.
pub fn scroll_into_view_selector(identifier: &str) -> String {
    let escaped = escape_selector(identifier);
    format!(
        "new UiScrollable(new UiSelector().scrollable(true)).setMaxSearchSwipes({SCROLL_SEARCH_SWIPES}).scrollIntoView(new UiSelector().resourceId(\"{escaped}\"))"
    )
}

fn escape_selector(identifier: &str) -> String {
    identifier.replace('\\', "\\\\").replace('"', "\\\"")
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
            // Sessions never launch the app implicitly: the coordinator
            // clears data and activates it as explicit, evidenced steps.
            "appium:autoLaunch": false,
            "appium:adbExecTimeout": ADB_EXEC_TIMEOUT_MS,
            "appium:uiautomator2ServerLaunchTimeout": ADB_EXEC_TIMEOUT_MS,
            "appium:uiautomator2ServerInstallTimeout": ADB_EXEC_TIMEOUT_MS,
        });
        if let Some(activity) = &self.app_activity {
            caps["appium:appActivity"] = Value::String(activity.clone());
        }
        caps
    }
}

/// What a bounded below-the-fold search came back with.
///
/// Three states, because two of them are not the same answer and the
/// callers differ on what to do about it: a search that reached the end is
/// evidence the control is not there, while a search that ran out of
/// budget is evidence of nothing at all.
#[derive(Debug)]
enum Probe {
    /// The search scrolled to the element.
    Found(Element),
    /// The search finished: nothing to scroll, or the end without it.
    NotThere,
    /// The search was cut off before it could answer.
    OutOfBudget,
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
        self.find_within(accessibility_id, SCROLL_PROBE_BUDGET)
            .await
    }

    /// [`Self::find`] with an explicit budget for the scrolling fallback.
    async fn find_within(
        &self,
        accessibility_id: &str,
        budget: Duration,
    ) -> Result<Element, AppiumError> {
        let not_visible = match self
            .session
            .find_element(
                Locator::UiAutomator,
                &resource_id_selector(accessibility_id),
            )
            .await
        {
            Ok(element) => return Ok(element),
            Err(not_visible) => not_visible,
        };
        // Not on screen is not the same as not there: scroll for it before
        // believing the app never rendered it. Bounded, because a lookup
        // has no deadline of its own to spend: without one this costs the
        // session's whole HTTP timeout every time a control is missing.
        match self.probe_below_the_fold(accessibility_id, budget).await? {
            Probe::Found(element) => Ok(element),
            // Nothing to scroll, or scrolled to the end without it: the
            // first failure is the one that describes the situation. A
            // failure of the search's own -- a dead session, a broken
            // transport -- came back from the probe as itself.
            Probe::NotThere => Err(not_visible.into()),
            // A search cut short answered nothing. A lookup has no deadline
            // to keep trying against, so it says exactly that instead of
            // repeating a `no such element` the search never confirmed.
            Probe::OutOfBudget => Err(AppiumError::ProbeUnfinished {
                accessibility_id: accessibility_id.to_owned(),
                budget,
            }),
        }
    }

    /// Scrolls the first scrollable container looking for
    /// `accessibility_id`, then returns the element **as the plain lookup
    /// sees it**.
    ///
    /// The scrolling selector cannot answer the question on its own.
    /// UiAutomator's `scrollIntoView` evaluates to the `UiScrollable` it
    /// was called on, so Appium replies `200` with the scrollable container
    /// -- even when the search swiped to the end and found nothing. Read as
    /// a lookup that is a control the app never rendered, and clicking the
    /// answer taps the middle of the list: a run drove the order form, was
    /// told a confirm button the screen does not have was there, tapped a
    /// field in the middle of the form instead, and then reported the order
    /// that was never created as a product bug.
    ///
    /// So the scrolling call is used for the scrolling, and the plain
    /// lookup decides.
    async fn scroll_to(&self, accessibility_id: &str) -> Result<Element, WebDriverError> {
        self.session
            .find_element(
                Locator::UiAutomator,
                &scroll_into_view_selector(accessibility_id),
            )
            .await?;
        self.session
            .find_element(
                Locator::UiAutomator,
                &resource_id_selector(accessibility_id),
            )
            .await
    }

    /// [`Self::scroll_to`] under a deadline of its own.
    ///
    /// Reports what the search did rather than only whether it worked: the
    /// caller is the one that knows whether being cut off is a failure.
    /// Every error is the driver's, not the screen's, and travels back as
    /// itself -- a probe that reported a dead session as an absent element
    /// would blame the app for the harness.
    async fn probe_below_the_fold(
        &self,
        accessibility_id: &str,
        budget: Duration,
    ) -> Result<Probe, AppiumError> {
        if budget.is_zero() {
            return Ok(Probe::OutOfBudget);
        }
        match tokio::time::timeout(budget, self.scroll_to(accessibility_id)).await {
            Ok(Ok(element)) => Ok(Probe::Found(element)),
            Ok(Err(e)) if e.is_no_such_element() => Ok(Probe::NotThere),
            Ok(Err(e)) => Err(e.into()),
            Err(_elapsed) => Ok(Probe::OutOfBudget),
        }
    }

    /// Whether an element with this accessibility id is present right now.
    ///
    /// Deliberately the plain lookup, with no scrolling: this probe answers
    /// "what is on the screen now", and callers ask it in loops -- which
    /// screen am I on, is the notice still up, can I go back yet. A probe
    /// that scrolled would answer the question by changing the thing it was
    /// asked about, leaving the screen somewhere the caller never navigated
    /// to, and would cost a full swipe search every time the answer is no.
    /// [`Self::wait_for`] is the primitive that looks below the fold.
    pub async fn exists(&self, accessibility_id: &str) -> Result<bool, AppiumError> {
        match self
            .session
            .find_element(
                Locator::UiAutomator,
                &resource_id_selector(accessibility_id),
            )
            .await
        {
            Ok(_) => Ok(true),
            Err(e) if e.is_no_such_element() => Ok(false),
            Err(e) => Err(e.into()),
        }
    }

    /// Waits until the element is present, bounded by `timeout`.
    ///
    /// Polling uses the plain lookup: it is cheap and it moves nothing,
    /// which matters because some waits are minutes long and a scrolling
    /// search on every poll would swipe the screen for the whole of one.
    /// But a control below the fold is absent from the accessibility tree
    /// until something scrolls to it, so the wait looks there too -- once,
    /// after the screen has had time to build, and once more before it
    /// gives up. Off-screen is a fixed property of a laid-out screen, so
    /// two probes settle it; one per poll would only thrash the device.
    ///
    /// Each probe is bounded by [`SCROLL_PROBE_BUDGET`], and never by more
    /// than `timeout` itself: the last one runs after the deadline, so a
    /// wait can outlive its deadline by at most what the wait was worth,
    /// rather than by the whole HTTP timeout of ten swipes.
    pub async fn wait_for(
        &self,
        accessibility_id: &str,
        timeout: Duration,
    ) -> Result<Element, AppiumError> {
        let started = tokio::time::Instant::now();
        let deadline = started + timeout;
        let probe_at = started + SCROLL_PROBE_DELAY.min(timeout);
        let budget = SCROLL_PROBE_BUDGET.min(timeout);
        let mut probed = false;
        loop {
            match self
                .session
                .find_element(
                    Locator::UiAutomator,
                    &resource_id_selector(accessibility_id),
                )
                .await
            {
                Ok(element) => return Ok(element),
                Err(e) if e.is_no_such_element() => {}
                Err(e) => return Err(e.into()),
            }
            let now = tokio::time::Instant::now();
            // Time this probe spends is time the polling loop does not get:
            // hold it inside the deadline as well as the budget. With none
            // of the deadline left it does not run at all, and the probe
            // the deadline branch makes below is the one that looks.
            let early_budget = budget.min(deadline.saturating_duration_since(now));
            let probing_now = !probed && now >= probe_at && !early_budget.is_zero();
            if probing_now {
                probed = true;
                // A wait spends probes out of its own deadline, and a short
                // one routinely cuts them off: being out of budget is not a
                // failure here, it is a reason to keep polling.
                if let Probe::Found(element) = self
                    .probe_below_the_fold(accessibility_id, early_budget)
                    .await?
                {
                    return Ok(element);
                }
            }
            if tokio::time::Instant::now() >= deadline {
                // A screen that finished building late is below the fold
                // only now; look once more before calling the control
                // missing, unless this very iteration already did.
                if !probing_now
                    && let Probe::Found(element) =
                        self.probe_below_the_fold(accessibility_id, budget).await?
                {
                    return Ok(element);
                }
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

    /// Accessibility label (`content-desc`) of an element: the state readout
    /// of container identifiers such as `order.status`.
    /// Reads what the element says: its accessibility description, or its
    /// text when there is none. A widget whose value is selectable text
    /// (an identity, an invoice) exposes it as text, and reading only the
    /// description would report such a screen as empty.
    pub async fn label_of(&self, accessibility_id: &str) -> Result<String, AppiumError> {
        let element = self.find(accessibility_id).await?;
        let described = self.session.attribute(&element, "content-desc").await?;
        if !described.trim().is_empty() {
            return Ok(described);
        }
        Ok(self.session.attribute(&element, "text").await?)
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

    /// Enables or disables both Wi-Fi and mobile data (`mobile: setConnectivity`).
    pub async fn set_connectivity(&self, enabled: bool) -> Result<(), AppiumError> {
        self.session
            .execute(
                "mobile: setConnectivity",
                vec![json!({ "wifi": enabled, "data": enabled })],
            )
            .await?;
        Ok(())
    }

    /// Rotates the device (`POST /orientation`).
    pub async fn set_orientation(&self, landscape: bool) -> Result<(), AppiumError> {
        let orientation = if landscape { "LANDSCAPE" } else { "PORTRAIT" };
        self.session
            .post("orientation", &json!({ "orientation": orientation }))
            .await?;
        Ok(())
    }

    /// Clears the app's data (`mobile: clearApp`), leaving it stopped.
    ///
    /// Clearing also revokes the runtime permissions the install granted, so
    /// they are granted again: otherwise the next launch opens on a system
    /// permission dialog covering the app, and every wait for a screen the
    /// app never got to show times out.
    pub async fn clear_app(&self) -> Result<(), AppiumError> {
        self.session
            .execute("mobile: clearApp", vec![json!({ "appId": self.package })])
            .await?;
        if let Err(e) = self
            .session
            .execute(
                "mobile: changePermissions",
                vec![json!({
                    "appPackage": self.package,
                    "permissions": "all",
                    "action": "grant",
                })],
            )
            .await
        {
            // An app with no runtime permissions, or a driver without the
            // command, must not fail the clear itself.
            warn!(error = %e, "cannot re-grant runtime permissions after clearing the app");
        }
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
                ResponseTemplate::new(200)
                    .set_body_json(json!({"value": {ELEMENT_KEY: "the-list"}})),
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
                ResponseTemplate::new(200)
                    .set_body_json(json!({"value": {ELEMENT_KEY: "the-list"}})),
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
                ResponseTemplate::new(200)
                    .set_body_json(json!({"value": {ELEMENT_KEY: "the-list"}})),
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
}
