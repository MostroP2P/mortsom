//! Platform-neutral ownership of actor sessions.
use mortsom_android_driver::{
    avd::Actor,
    farm::{DeviceFarm, TeardownItem},
    process::{CommandRunner, TokioRunner},
    ui::UiSession,
};
use mortsom_desktop_driver::{Platform, linux::LinuxSession, server::WebServer, web::WebSession};
use std::{path::PathBuf, sync::Arc};

/// Built client artifact and selected runtime.
#[derive(Debug, Clone)]
pub struct DesktopOptions {
    /// Linux or Web; Android uses the established device farm.
    pub platform: Platform,
    /// Native executable or Web build directory.
    pub artifact: PathBuf,
    /// Run-owned bundle, independent of later changes to the source build.
    pub bundle: Arc<crate::fingerprint::CapturedBundle>,
    /// Xvfb executable for private Linux displays.
    pub xvfb: PathBuf,
    /// ChromeDriver executable.
    pub chromedriver: PathBuf,
    /// Chromium executable.
    pub browser: PathBuf,
}

impl DesktopOptions {
    /// Refuses a changed bundle or an artifact outside its owned snapshot.
    pub async fn verify(&self) -> Result<(), String> {
        let contained = match self.platform {
            Platform::Linux => {
                self.artifact.parent() == Some(self.bundle.root())
                    && self.artifact.file_name().is_some()
            }
            Platform::Web => self.artifact == self.bundle.root(),
            Platform::Android => false,
        };
        if !contained {
            return Err("desktop artifact does not belong to the captured bundle".into());
        }
        crate::desktop::validate_artifact(self.platform, &self.artifact, &crate::doctor::RealHost)?;
        let bundle = Arc::clone(&self.bundle);
        tokio::task::spawn_blocking(move || bundle.verify())
            .await
            .map_err(|_| "bundle verification task failed".to_string())?
            .map_err(|error| error.to_string())
    }
}

pub(super) enum Farm {
    Android(DeviceFarm),
    Desktop {
        sessions: Vec<(Actor, Arc<dyn UiSession>)>,
        server: Option<WebServer>,
        bundle: Arc<crate::fingerprint::CapturedBundle>,
    },
}

impl Farm {
    pub(super) async fn desktop(options: &DesktopOptions) -> Result<Self, String> {
        options.verify().await?;
        let server = if options.platform == Platform::Web {
            Some(
                WebServer::start_owned(&options.artifact, Arc::clone(&options.bundle))
                    .await
                    .map_err(|e| e.to_string())?,
            )
        } else {
            None
        };
        let mut sessions = Vec::new();
        for actor in Actor::ALL {
            let session: Arc<dyn UiSession> = match &server {
                Some(server) => Arc::new(WebSession::new(
                    Arc::new(TokioRunner),
                    server.url().into(),
                    options.chromedriver.clone(),
                    options.browser.clone(),
                )),
                None => Arc::new(LinuxSession::new(
                    Arc::new(TokioRunner),
                    options.artifact.clone(),
                    options.xvfb.clone(),
                )),
            };
            sessions.push((actor, session));
        }
        Ok(Self::Desktop {
            sessions,
            server,
            bundle: Arc::clone(&options.bundle),
        })
    }

    pub(super) fn sessions(&self) -> Vec<(Actor, Arc<dyn UiSession>)> {
        match self {
            Self::Android(farm) => farm
                .devices()
                .iter()
                .map(|d| (d.actor, Arc::clone(&d.session)))
                .collect(),
            Self::Desktop { sessions, .. } => sessions.clone(),
        }
    }

    pub(super) fn version(&self) -> String {
        match self {
            Self::Android(farm) => farm.device(Actor::Alice).version.version_name.clone(),
            Self::Desktop { .. } => "v2-desktop".into(),
        }
    }

    pub(super) async fn tear_down(self, runner: &dyn CommandRunner) -> Vec<TeardownItem> {
        match self {
            Self::Android(farm) => farm.tear_down(runner).await,
            Self::Desktop {
                sessions,
                server,
                bundle: _bundle,
            } => {
                let mut items = Vec::new();
                for (actor, session) in sessions {
                    let result = session.end().await;
                    items.push(TeardownItem {
                        resource: format!("{actor} desktop"),
                        ok: result.is_ok(),
                        error: result.err().map(|e| e.to_string()),
                    });
                }
                if let Some(server) = server {
                    let result = server.stop().await;
                    items.push(TeardownItem {
                        resource: "Web bundle server".into(),
                        ok: result.is_ok(),
                        error: result.err().map(|error| error.to_string()),
                    });
                }
                items
            }
        }
    }
}
