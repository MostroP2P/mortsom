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
/// and cost a minute. A wait spends at most this much on a probe, and never
/// more than the wait itself is worth.
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
        match self
            .session
            .find_element(
                Locator::UiAutomator,
                &resource_id_selector(accessibility_id),
            )
            .await
        {
            Ok(element) => Ok(element),
            // Not on screen is not the same as not there: scroll for it
            // before believing the app never rendered it.
            Err(not_visible) => match self.scroll_to(accessibility_id).await {
                Ok(element) => Ok(element),
                // Nothing to scroll, or scrolled to the end without it: the
                // first failure is the one that describes the situation.
                Err(e) if e.is_no_such_element() => Err(not_visible.into()),
                // The search never got to answer: a dead session or a
                // broken transport is its own failure, not a missing
                // control, and reporting it as one blames the app.
                Err(e) => Err(e.into()),
            },
        }
    }

    /// Scrolls the first scrollable container until the element comes into
    /// view, and returns it. Fails when there is nothing to scroll or the
    /// search reaches the end without finding it.
    async fn scroll_to(&self, accessibility_id: &str) -> Result<Element, WebDriverError> {
        self.session
            .find_element(
                Locator::UiAutomator,
                &scroll_into_view_selector(accessibility_id),
            )
            .await
    }

    /// [`Self::scroll_to`] under a deadline of its own, for waits.
    ///
    /// `Ok(None)` is the one thing a wait may keep waiting through: the
    /// search reached the end without the element, found nothing to scroll,
    /// or ran out of `budget`. Every other failure is the driver's, not the
    /// screen's, and travels back to the caller as itself -- a wait that
    /// reported a dead session as `WaitTimeout` would blame the app for the
    /// harness.
    async fn probe_below_the_fold(
        &self,
        accessibility_id: &str,
        budget: Duration,
    ) -> Result<Option<Element>, AppiumError> {
        if budget.is_zero() {
            return Ok(None);
        }
        match tokio::time::timeout(budget, self.scroll_to(accessibility_id)).await {
            Ok(Ok(element)) => Ok(Some(element)),
            Ok(Err(e)) if e.is_no_such_element() => Ok(None),
            Ok(Err(e)) => Err(e.into()),
            Err(_out_of_budget) => Ok(None),
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
                if let Some(element) = self
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
                    && let Some(element) =
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

    /// Focuses, clears, then types into an element found by accessibility
    /// id. The text may be a secret (NWC URI, mnemonic) and is never logged.
    ///
    /// The tap is not decoration. Appium's set-text reaches Flutter as
    /// `SemanticsAction.setText`, which a field without input focus ignores
    /// -- while still answering `200`, so the driver believes it typed. The
    /// one field in the app that took text this way was the one declared
    /// `autofocus: true`; every field in the order form is left unfocused,
    /// stayed empty through a whole form fill, and the run reported the
    /// order the app never accepted as a product bug. A field takes text
    /// through the keyboard, and the keyboard goes to whatever has focus:
    /// tapping it first is what a user does.
    #[tracing::instrument(skip(self, text), fields(id = accessibility_id))]
    pub async fn type_text(&self, accessibility_id: &str, text: &str) -> Result<(), AppiumError> {
        let element = self.find(accessibility_id).await?;
        self.session.click(&element).await?;
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
mod tests;
