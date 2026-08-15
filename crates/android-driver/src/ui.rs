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
