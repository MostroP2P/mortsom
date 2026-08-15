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
    /// End the session.
    async fn end(&self) -> Result<(), AppiumError>;
}

#[async_trait]
impl UiSession for AppiumSession {
    async fn exists(&self, id: &str) -> Result<bool, AppiumError> {
        AppiumSession::exists(self, id).await
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
    }

    impl ScriptedUi {
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
    }

    #[async_trait]
    impl UiSession for ScriptedUi {
        async fn exists(&self, id: &str) -> Result<bool, AppiumError> {
            Ok(self.has(id))
        }
        async fn wait_for(&self, id: &str, _: Duration) -> Result<(), AppiumError> {
            if self.has(id) {
                Ok(())
            } else {
                Err(AppiumError::WaitTimeout {
                    accessibility_id: id.into(),
                    condition: "not found",
                    timeout: Duration::ZERO,
                })
            }
        }
        async fn wait_gone(&self, _: &str, _: Duration) -> Result<(), AppiumError> {
            Ok(())
        }
        async fn tap(&self, id: &str) -> Result<(), AppiumError> {
            self.calls.lock().unwrap().push(format!("tap {id}"));
            if let Some(appear) = self.on_tap.lock().unwrap().get(id).cloned() {
                self.present.lock().unwrap().extend(appear);
            }
            if let Some(hide) = self.on_tap_hide.lock().unwrap().get(id).cloned() {
                self.present.lock().unwrap().retain(|p| !hide.contains(p));
            }
            Ok(())
        }
        async fn type_text(&self, id: &str, text: &str) -> Result<(), AppiumError> {
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
        async fn is_enabled(&self, _: &str) -> Result<bool, AppiumError> {
            Ok(true)
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
            Ok(())
        }
    }
}
