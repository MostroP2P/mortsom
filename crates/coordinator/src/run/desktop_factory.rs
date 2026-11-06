//! Injectable construction of desktop actors and their shared Web server.
use super::DesktopOptions;
use async_trait::async_trait;
use mortsom_common::Actor;
use mortsom_desktop_driver::{Platform, linux::LinuxSession, server::WebServer, web::WebSession};
use mortsom_ui_driver::{process::TokioRunner, ui::UiSession};
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
    /// Interruptions the sessions it makes can be put through. Linux and Web
    /// sessions cannot be sent to the background, cut off the network or
    /// rotated; none of them is offered.
    fn interruptions(&self) -> &'static [mortsom_scenario_kit::registry::Need] {
        &[]
    }
}
/// Native processes and Chromium reached only through their public UI.
pub struct RealDesktopFactory;
#[async_trait]
impl DesktopFactory for RealDesktopFactory {
    async fn server(&self, options: &DesktopOptions) -> Result<Box<dyn DesktopServer>, String> {
        let origin = options
            .recovery
            .as_ref()
            .and_then(|journal| journal.snapshot().origin);
        let port = recovery_port(options.resume, origin.as_deref())?;
        let server =
            WebServer::start_owned_on_port(&options.artifact, Arc::clone(&options.bundle), port)
                .await
                .map_err(|cause| cause.to_string())?;
        Ok(Box::new(RealServer {
            url: server.url().into(),
            server: Some(server),
        }))
    }
    fn session(
        &self,
        actor: Actor,
        options: &DesktopOptions,
        origin: Option<&str>,
    ) -> Result<Arc<dyn UiSession>, String> {
        let storage = options.recovery.as_ref().map(|journal| {
            let root = journal.root().join("actors").join(actor.as_str());
            if options.resume {
                mortsom_desktop_driver::storage::ActorStorage::Resume(root)
            } else {
                mortsom_desktop_driver::storage::ActorStorage::Fresh(root)
            }
        });
        match options.platform {
            Platform::Cli => Err("the cli profile is not available yet".into()),
            Platform::Web => {
                let session = WebSession::new(
                    Arc::new(TokioRunner),
                    origin.ok_or("Web server origin missing")?.into(),
                    options.chromedriver.clone(),
                    options.browser.clone(),
                );
                Ok(Arc::new(match storage {
                    Some(storage) => session.with_storage(storage),
                    None => session,
                }))
            }
            Platform::Linux => {
                let session = LinuxSession::new(
                    Arc::new(TokioRunner),
                    options.artifact.clone(),
                    options.xvfb.clone(),
                );
                Ok(Arc::new(match storage {
                    Some(storage) => session.with_storage(storage),
                    None => session,
                }))
            }
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

/// A recovered browser must bind its original origin before opening any profile.
fn recovery_port(resume: bool, origin: Option<&str>) -> Result<u16, String> {
    let Some(origin) = origin else {
        return if resume {
            Err("retained Web origin missing".into())
        } else {
            Ok(0)
        };
    };
    let port = origin
        .strip_prefix("http://127.0.0.1:")
        .ok_or("invalid recovery Web origin")?
        .parse::<u16>()
        .map_err(|_| "invalid recovery Web port")?;
    if port == 0 {
        return Err("retained Web origin cannot use an ephemeral port".into());
    }
    Ok(port)
}

#[cfg(test)]
mod tests {
    use super::recovery_port;
    #[test]
    fn resumed_browser_refuses_missing_or_nonpersistent_origins() {
        assert_eq!(recovery_port(false, None), Ok(0));
        assert_eq!(recovery_port(true, Some("http://127.0.0.1:8123")), Ok(8123));
        for origin in [
            None,
            Some("http://127.0.0.1:0"),
            Some("http://localhost:8123"),
            Some("http://127.0.0.1:8123/path"),
        ] {
            assert!(recovery_port(true, origin).is_err());
        }
    }
}
