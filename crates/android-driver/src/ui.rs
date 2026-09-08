//! UI automation boundary consumed by adapters and the coordinator. The
//! real implementation is Appium/UiAutomator2; tests inject fakes.

use std::path::Path;
use std::time::Duration;

use async_trait::async_trait;

use crate::appium::{AppiumError, AppiumServer, AppiumSession, SessionSpec};
use crate::process::CommandRunner;

/// A UI automation session bound to one device and one app.
#[async_trait]
pub trait UiSession: Send + Sync {
    /// Whether an element with this accessibility id exists now.
    async fn exists(&self, accessibility_id: &str) -> Result<bool, AppiumError>;
    /// Whether the element is on the screen or reachable by scrolling to
    /// it. Ask this about a list; it may swipe the screen to answer.
    /// Defaults to [`Self::exists`] for sessions with nothing to scroll.
    async fn exists_anywhere(&self, accessibility_id: &str) -> Result<bool, AppiumError> {
        self.exists(accessibility_id).await
    }
    /// Waits for an element to appear.
    async fn wait_for(&self, accessibility_id: &str, timeout: Duration) -> Result<(), AppiumError>;
    /// Waits for an element to disappear.
    async fn wait_gone(&self, accessibility_id: &str, timeout: Duration)
    -> Result<(), AppiumError>;
    /// Taps an element.
    async fn tap(&self, accessibility_id: &str) -> Result<(), AppiumError>;
    /// Clears and types into an element. Text is never logged.
    async fn type_text(&self, accessibility_id: &str, text: &str) -> Result<(), AppiumError>;
    /// Visible text of an element.
    async fn text_of(&self, accessibility_id: &str) -> Result<String, AppiumError>;
    /// Whether an element is enabled.
    async fn is_enabled(&self, accessibility_id: &str) -> Result<bool, AppiumError>;
    /// Accessibility label (state readout) of an element.
    async fn label_of(&self, accessibility_id: &str) -> Result<String, AppiumError>;
    /// PNG screenshot.
    async fn screenshot_png(&self) -> Result<Vec<u8>, AppiumError>;
    /// Accessibility hierarchy XML.
    async fn page_source(&self) -> Result<String, AppiumError>;
    /// Background for `secs` seconds and return.
    async fn background(&self, secs: u64) -> Result<(), AppiumError>;
    /// Force-stop the app.
    async fn terminate_app(&self) -> Result<(), AppiumError>;
    /// Bring the app to the foreground, launching if needed.
    async fn activate_app(&self) -> Result<(), AppiumError>;
    /// Clear app data.
    async fn clear_app(&self) -> Result<(), AppiumError>;
    /// Enable or disable network connectivity (Wi-Fi and mobile data).
    async fn set_connectivity(&self, enabled: bool) -> Result<(), AppiumError>;
    /// Rotate the device to landscape or portrait.
    async fn set_orientation(&self, landscape: bool) -> Result<(), AppiumError>;
    /// Dismiss the soft keyboard if one is up.
    async fn hide_keyboard(&self) -> Result<(), AppiumError>;
    /// End the session.
    async fn end(&self) -> Result<(), AppiumError>;
}

#[async_trait]
impl UiSession for AppiumSession {
    async fn exists(&self, id: &str) -> Result<bool, AppiumError> {
        AppiumSession::exists(self, id).await
    }
    async fn exists_anywhere(&self, id: &str) -> Result<bool, AppiumError> {
        AppiumSession::exists_anywhere(self, id).await
    }
    async fn wait_for(&self, id: &str, timeout: Duration) -> Result<(), AppiumError> {
        AppiumSession::wait_for(self, id, timeout).await.map(|_| ())
    }
    async fn wait_gone(&self, id: &str, timeout: Duration) -> Result<(), AppiumError> {
        AppiumSession::wait_gone(self, id, timeout).await
    }
    async fn tap(&self, id: &str) -> Result<(), AppiumError> {
        AppiumSession::tap(self, id).await
    }
    async fn type_text(&self, id: &str, text: &str) -> Result<(), AppiumError> {
        AppiumSession::type_text(self, id, text).await
    }
    async fn text_of(&self, id: &str) -> Result<String, AppiumError> {
        AppiumSession::text_of(self, id).await
    }
    async fn is_enabled(&self, id: &str) -> Result<bool, AppiumError> {
        AppiumSession::is_enabled(self, id).await
    }
    async fn label_of(&self, id: &str) -> Result<String, AppiumError> {
        AppiumSession::label_of(self, id).await
    }
    async fn screenshot_png(&self) -> Result<Vec<u8>, AppiumError> {
        AppiumSession::screenshot_png(self).await
    }
    async fn page_source(&self) -> Result<String, AppiumError> {
        AppiumSession::page_source(self).await
    }
    async fn background(&self, secs: u64) -> Result<(), AppiumError> {
        AppiumSession::background(self, secs).await
    }
    async fn terminate_app(&self) -> Result<(), AppiumError> {
        AppiumSession::terminate_app(self).await
    }
    async fn activate_app(&self) -> Result<(), AppiumError> {
        AppiumSession::activate_app(self).await
    }
    async fn clear_app(&self) -> Result<(), AppiumError> {
        AppiumSession::clear_app(self).await
    }
    async fn set_connectivity(&self, enabled: bool) -> Result<(), AppiumError> {
        AppiumSession::set_connectivity(self, enabled).await
    }
    async fn hide_keyboard(&self) -> Result<(), AppiumError> {
        AppiumSession::hide_keyboard(self).await
    }
    async fn set_orientation(&self, landscape: bool) -> Result<(), AppiumError> {
        AppiumSession::set_orientation(self, landscape).await
    }
    async fn end(&self) -> Result<(), AppiumError> {
        AppiumSession::end(self).await
    }
}

/// A running automation server that hands out sessions.
#[async_trait]
pub trait UiServer: Send + Sync {
    /// Creates a session for one device.
    async fn create_session(&self, spec: &SessionSpec) -> Result<Box<dyn UiSession>, AppiumError>;
    /// Stops the server.
    async fn stop(&self) -> Result<(), AppiumError>;
}

/// Starts automation servers.
#[async_trait]
pub trait UiDriver: Send + Sync {
    /// Starts a server on `port`, logging into `log_dir`.
    async fn start_server(
        &self,
        runner: &dyn CommandRunner,
        port: u16,
        log_dir: &Path,
        ready_timeout: Duration,
    ) -> Result<Box<dyn UiServer>, AppiumError>;
}

/// The real Appium driver: locates the `appium` executable at construction.
#[derive(Debug, Clone)]
pub struct AppiumDriver {
    appium: std::path::PathBuf,
}

impl AppiumDriver {
    /// Uses this `appium` executable.
    pub fn new(appium: std::path::PathBuf) -> Self {
        Self { appium }
    }
}

#[async_trait]
impl UiDriver for AppiumDriver {
    async fn start_server(
        &self,
        runner: &dyn CommandRunner,
        port: u16,
        log_dir: &Path,
        ready_timeout: Duration,
    ) -> Result<Box<dyn UiServer>, AppiumError> {
        let server =
            AppiumServer::start(runner, &self.appium, port, log_dir, ready_timeout).await?;
        Ok(Box::new(server))
    }
}

#[async_trait]
impl UiServer for AppiumServer {
    async fn create_session(&self, spec: &SessionSpec) -> Result<Box<dyn UiSession>, AppiumError> {
        let session = AppiumSession::create(&self.base_url(), spec).await?;
        Ok(Box::new(session))
    }
    async fn stop(&self) -> Result<(), AppiumError> {
        AppiumServer::stop(self).await
    }
}

/// Scripted UI session for adapter tests in this and other crates.
#[cfg(any(test, feature = "testing"))]
#[allow(clippy::unwrap_used, clippy::expect_used)]
pub mod tests_support {
    use std::collections::BTreeMap;
    use std::sync::Mutex;
    use std::time::Duration;

    use async_trait::async_trait;

    use super::UiSession;
    use crate::appium::AppiumError;

    /// Scripted UI: elements exist when listed (dynamic changes via `on_tap`),
    /// labels answer from a map, and every action is recorded.
    #[derive(Default)]
    pub struct ScriptedUi {
        present: Mutex<Vec<String>>,
        labels: Mutex<BTreeMap<String, String>>,
        calls: Mutex<Vec<String>>,
        on_tap: Mutex<BTreeMap<String, Vec<String>>>,
        /// Elements that disappear after `id` is tapped (navigation away).
        on_tap_hide: Mutex<BTreeMap<String, Vec<String>>>,
        /// Label changes triggered by a tap.
        on_tap_relabel: Mutex<BTreeMap<String, (String, String)>>,
        /// Elements rendered below the fold: only a scrolling look finds them.
        below: Mutex<Vec<String>>,
        /// Elements the app has not drawn yet, with the number of probes
        /// still to miss them.
        pending: Mutex<BTreeMap<String, u32>>,
        /// Elements the soft keyboard covers while it is up.
        under_keyboard: Mutex<Vec<String>>,
        /// Whether typing has raised the keyboard.
        keyboard_up: Mutex<bool>,
        /// Taps that land while the app settles and do nothing, per id.
        swallowed: Mutex<BTreeMap<String, u32>>,
        /// Controls the app has greyed out while it works.
        disabled: Mutex<Vec<String>>,
        end_failures: Mutex<u32>,
    }

    impl ScriptedUi {
        /// Injects transient shutdown failures while retaining this fake session.
        pub fn fail_end_times(&self, failures: u32) {
            *self.end_failures.lock().unwrap() = failures;
        }
        /// Marks `ids` as present.
        pub fn present(&self, ids: &[&str]) {
            self.present
                .lock()
                .unwrap()
                .extend(ids.iter().map(|s| (*s).to_owned()));
        }
        /// Sets the accessibility label answered for `id`.
        pub fn label(&self, id: &str, value: &str) {
            self.labels.lock().unwrap().insert(id.into(), value.into());
        }
        /// Elements that appear after `id` is tapped.
        pub fn on_tap(&self, id: &str, appear: &[&str]) {
            self.on_tap
                .lock()
                .unwrap()
                .insert(id.into(), appear.iter().map(|s| (*s).to_owned()).collect());
        }
        /// After `id` is tapped, `target`'s label becomes `value`.
        pub fn on_tap_relabel(&self, id: &str, target: &str, value: &str) {
            self.on_tap_relabel
                .lock()
                .unwrap()
                .insert(id.into(), (target.into(), value.into()));
        }

        /// Elements that disappear after `id` is tapped.
        pub fn on_tap_hide(&self, id: &str, hide: &[&str]) {
            self.on_tap_hide
                .lock()
                .unwrap()
                .insert(id.into(), hide.iter().map(|s| (*s).to_owned()).collect());
        }
        /// Every action performed, in order.
        pub fn calls(&self) -> Vec<String> {
            self.calls.lock().unwrap().clone()
        }
        /// Whether `id` is present now.
        pub fn has(&self, id: &str) -> bool {
            self.present.lock().unwrap().iter().any(|p| p == id)
        }

        /// Marks `ids` as rendered but off-screen: a plain probe does not
        /// see them, while a wait and a list probe both scroll and do.
        pub fn below_the_fold(&self, ids: &[&str]) {
            self.below
                .lock()
                .unwrap()
                .extend(ids.iter().map(|s| (*s).to_owned()));
        }

        /// Makes `id` miss its next `probes` looks before it is there.
        ///
        /// A cold start is exactly this: the session opens before the app
        /// has drawn its first frame, so an element the screen will show is
        /// briefly absent. A double that is either present or not cannot
        /// tell a screen apart from a screen that has not arrived.
        pub fn appears_after(&self, id: &str, probes: u32) {
            self.pending.lock().unwrap().insert(id.into(), probes);
        }

        /// Marks `ids` as the part of the screen the soft keyboard covers.
        ///
        /// Typing raises it and `hide_keyboard` puts it down. A covered
        /// element is gone, not merely off-screen: the app's semantics stop
        /// at the shrunken viewport, so no amount of scrolling finds it --
        /// which is why a double that only knows "above" and "below the
        /// fold" cannot express this.
        pub fn keyboard_over(&self, ids: &[&str]) {
            self.under_keyboard
                .lock()
                .unwrap()
                .extend(ids.iter().map(|s| (*s).to_owned()));
        }

        /// Makes the next `taps` on `id` land and do nothing.
        ///
        /// An app rebuilding a screen -- after a relaunch, or as a keyboard
        /// closes under the button -- takes the tap and acts on none of it.
        /// The control is there and the driver hits it, so a double that
        /// always acts on a tap cannot express this.
        pub fn swallows_taps(&self, id: &str, taps: u32) {
            self.swallowed.lock().unwrap().insert(id.into(), taps);
        }

        /// Greys `ids` out: on screen, but not accepting a press.
        ///
        /// A form that has been submitted and is waiting on the network
        /// looks like this -- the button is still drawn, and pressing it
        /// again does nothing the app would honour. A double that answers
        /// every control as enabled cannot tell that apart from a form
        /// that was never submitted at all.
        pub fn disables(&self, ids: &[&str]) {
            self.disabled
                .lock()
                .unwrap()
                .extend(ids.iter().map(|s| (*s).to_owned()));
        }

        /// Whether this tap is swallowed, counting it if so.
        fn swallows(&self, id: &str) -> bool {
            let mut swallowed = self.swallowed.lock().unwrap();
            match swallowed.get_mut(id) {
                Some(left) if *left > 0 => {
                    *left -= 1;
                    true
                }
                _ => false,
            }
        }

        /// Whether the keyboard is covering `id` right now.
        fn covered(&self, id: &str) -> bool {
            *self.keyboard_up.lock().unwrap()
                && self.under_keyboard.lock().unwrap().iter().any(|p| p == id)
        }

        /// Whether `id` is anywhere on the current screen, fold or no fold.
        pub fn has_anywhere(&self, id: &str) -> bool {
            self.has(id) || self.below.lock().unwrap().iter().any(|p| p == id)
        }
    }

    #[async_trait]
    impl UiSession for ScriptedUi {
        async fn exists(&self, id: &str) -> Result<bool, AppiumError> {
            let drawn = {
                let mut pending = self.pending.lock().unwrap();
                match pending.get_mut(id) {
                    Some(left) if *left > 0 => {
                        *left -= 1;
                        false
                    }
                    Some(_) => {
                        pending.remove(id);
                        true
                    }
                    None => true,
                }
            };
            Ok(drawn && self.has(id) && !self.covered(id))
        }
        async fn exists_anywhere(&self, id: &str) -> Result<bool, AppiumError> {
            Ok(self.has_anywhere(id) && !self.covered(id))
        }
        async fn wait_for(&self, id: &str, _: Duration) -> Result<(), AppiumError> {
            // A wait scrolls before it gives up, so it sees below the fold --
            // but not under the keyboard, which is not a scrolling problem.
            if self.has_anywhere(id) && !self.covered(id) {
                Ok(())
            } else {
                Err(AppiumError::WaitTimeout {
                    accessibility_id: id.into(),
                    condition: "not found",
                    timeout: Duration::ZERO,
                })
            }
        }
        async fn wait_gone(&self, id: &str, _: Duration) -> Result<(), AppiumError> {
            if self.has(id) {
                Err(AppiumError::WaitTimeout {
                    accessibility_id: id.into(),
                    condition: "still present",
                    timeout: Duration::ZERO,
                })
            } else {
                Ok(())
            }
        }
        async fn tap(&self, id: &str) -> Result<(), AppiumError> {
            // A control the keyboard covers cannot be tapped: it is not on
            // the screen to be hit. A double that let it through would pass
            // a flow the real driver cannot perform.
            if self.covered(id) {
                return Err(AppiumError::WaitTimeout {
                    accessibility_id: id.into(),
                    condition: "under the keyboard",
                    timeout: Duration::ZERO,
                });
            }
            self.calls.lock().unwrap().push(format!("tap {id}"));
            if self.swallows(id) {
                return Ok(());
            }
            if let Some(appear) = self.on_tap.lock().unwrap().get(id).cloned() {
                self.present.lock().unwrap().extend(appear);
            }
            if let Some(hide) = self.on_tap_hide.lock().unwrap().get(id).cloned() {
                self.present.lock().unwrap().retain(|p| !hide.contains(p));
            }
            if let Some((target, value)) = self.on_tap_relabel.lock().unwrap().get(id).cloned() {
                self.labels.lock().unwrap().insert(target, value);
            }
            Ok(())
        }
        async fn hide_keyboard(&self) -> Result<(), AppiumError> {
            self.calls.lock().unwrap().push("hide keyboard".into());
            *self.keyboard_up.lock().unwrap() = false;
            Ok(())
        }
        async fn type_text(&self, id: &str, text: &str) -> Result<(), AppiumError> {
            *self.keyboard_up.lock().unwrap() = true;
            // Record the length only: secrets never reach logs, not even fakes'.
            self.calls
                .lock()
                .unwrap()
                .push(format!("type {id} ({} chars)", text.len()));
            Ok(())
        }
        async fn text_of(&self, _: &str) -> Result<String, AppiumError> {
            Ok(String::new())
        }
        async fn is_enabled(&self, id: &str) -> Result<bool, AppiumError> {
            Ok(!self.disabled.lock().unwrap().iter().any(|d| d == id))
        }
        async fn label_of(&self, id: &str) -> Result<String, AppiumError> {
            Ok(self
                .labels
                .lock()
                .unwrap()
                .get(id)
                .cloned()
                .unwrap_or_default())
        }
        async fn screenshot_png(&self) -> Result<Vec<u8>, AppiumError> {
            Ok(vec![])
        }
        async fn page_source(&self) -> Result<String, AppiumError> {
            // Hierarchy dump listing every present id as a resource-id.
            Ok(self
                .present
                .lock()
                .unwrap()
                .iter()
                .map(|id| format!("<node resource-id=\"{id}\"/>"))
                .collect())
        }
        async fn background(&self, _: u64) -> Result<(), AppiumError> {
            Ok(())
        }
        async fn terminate_app(&self) -> Result<(), AppiumError> {
            Ok(())
        }
        async fn activate_app(&self) -> Result<(), AppiumError> {
            Ok(())
        }
        async fn clear_app(&self) -> Result<(), AppiumError> {
            Ok(())
        }
        async fn set_connectivity(&self, _: bool) -> Result<(), AppiumError> {
            Ok(())
        }
        async fn set_orientation(&self, _: bool) -> Result<(), AppiumError> {
            Ok(())
        }
        async fn end(&self) -> Result<(), AppiumError> {
            let mut failures = self.end_failures.lock().unwrap();
            if *failures > 0 {
                *failures -= 1;
                self.calls.lock().unwrap().push("end failed".into());
                return Err(AppiumError::WebDriver(
                    crate::webdriver::WebDriverError::Transport("injected cleanup failure".into()),
                ));
            }
            self.calls.lock().unwrap().push("end session".into());
            Ok(())
        }
    }
}
