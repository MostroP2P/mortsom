//! Injectable construction of desktop actors and their shared Web server.
use super::DesktopOptions;
use async_trait::async_trait;
use mortsom_android_driver::{avd::Actor, process::TokioRunner, ui::UiSession};
use mortsom_desktop_driver::{Platform, linux::LinuxSession, server::WebServer, web::WebSession};
use std::sync::Arc;

/// Owns the Web origin and all request tasks used by a farm.
#[async_trait]
pub trait DesktopServer: Send {
    /// Bound loopback origin.
    fn url(&self) -> &str;
    /// Stops requests before releasing bundle ownership.
    async fn stop(&mut self) -> Result<(), String>;
}
/// External desktop construction boundary; fake implementations start no processes.
#[async_trait]
pub trait DesktopFactory: Send + Sync {
    /// Creates the owned static server for a Web farm.
    async fn server(&self, options: &DesktopOptions) -> Result<Box<dyn DesktopServer>, String>;
    /// Creates one independent actor session descriptor.
    fn session(
        &self,
        actor: Actor,
        options: &DesktopOptions,
        origin: Option<&str>,
    ) -> Result<Arc<dyn UiSession>, String>;
}
/// Native processes and Chromium reached only through their public UI.
pub struct RealDesktopFactory;
#[async_trait]
impl DesktopFactory for RealDesktopFactory {
    async fn server(&self, options: &DesktopOptions) -> Result<Box<dyn DesktopServer>, String> {
        let server = WebServer::start_owned(&options.artifact, Arc::clone(&options.bundle))
            .await
            .map_err(|cause| cause.to_string())?;
        Ok(Box::new(RealServer {
            url: server.url().into(),
            server: Some(server),
        }))
    }
    fn session(
        &self,
        _: Actor,
        options: &DesktopOptions,
        origin: Option<&str>,
    ) -> Result<Arc<dyn UiSession>, String> {
        match options.platform {
            Platform::Web => Ok(Arc::new(WebSession::new(
                Arc::new(TokioRunner),
                origin.ok_or("Web server origin missing")?.into(),
                options.chromedriver.clone(),
                options.browser.clone(),
            ))),
            Platform::Linux => Ok(Arc::new(LinuxSession::new(
                Arc::new(TokioRunner),
                options.artifact.clone(),
                options.xvfb.clone(),
            ))),
            Platform::Android => Err("desktop factory cannot construct Android clients".into()),
        }
    }
}
struct RealServer {
    url: String,
    server: Option<WebServer>,
}
#[async_trait]
impl DesktopServer for RealServer {
    fn url(&self) -> &str {
        &self.url
    }
    async fn stop(&mut self) -> Result<(), String> {
        if let Some(server) = self.server.as_mut() {
            server.shutdown().await.map_err(|cause| cause.to_string())?;
            self.server = None;
        }
        Ok(())
    }
}
