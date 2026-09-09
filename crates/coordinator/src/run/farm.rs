//! Platform-neutral ownership of actor sessions.
use super::{DesktopFactory, DesktopServer};
use mortsom_android_driver::{
    avd::Actor,
    farm::{DeviceFarm, TeardownItem},
    process::CommandRunner,
    ui::UiSession,
};
use mortsom_desktop_driver::Platform;
use std::{path::PathBuf, sync::Arc};

/// Built client artifact and selected runtime.
#[derive(Debug, Clone)]
pub struct DesktopOptions {
    /// Durable actor/bundle ownership for this invocation.
    pub recovery: Option<Arc<crate::recovery_store::FileJournal>>,
    /// Resume existing identities; never run first-launch bootstrap.
    pub resume: bool,
    /// Linux or Web; Android uses the established device farm.
    pub platform: Platform,
    /// Native executable or Web build directory.
    pub artifact: PathBuf,
    /// Run-owned bundle, independent of later changes to the source build.
    pub bundle: Arc<crate::fingerprint::CapturedBundle>,
    /// Source/build facts associated with the captured contents.
    pub provenance: crate::provenance::BuildProvenance,
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
        server: Option<Box<dyn DesktopServer>>,
        bundle: Arc<crate::fingerprint::CapturedBundle>,
        app_version: Option<String>,
    },
}

impl Farm {
    pub(super) async fn desktop(
        options: &DesktopOptions,
        factory: &dyn DesktopFactory,
    ) -> Result<Self, String> {
        options.verify().await?;
        let mut server = if options.platform == Platform::Web {
            Some(factory.server(options).await?)
        } else {
            None
        };
        let mut sessions = Vec::new();
        for actor in Actor::ALL {
            match factory.session(actor, options, server.as_ref().map(|server| server.url())) {
                Ok(session) => sessions.push((actor, session)),
                Err(cause) => {
                    let mut failures = vec![cause];
                    for (actor, session) in &sessions {
                        if let Err(cause) = end_session(session.as_ref()).await {
                            failures.push(format!("{actor} cleanup: {cause}"));
                        }
                    }
                    if let Some(server) = server.as_mut()
                        && let Err(cause) = server.stop().await
                    {
                        failures.push(format!("Web server cleanup: {cause}"));
                    }
                    return Err(failures.join("; "));
                }
            }
        }
        Ok(Self::Desktop {
            sessions,
            server,
            bundle: Arc::clone(&options.bundle),
            app_version: options.provenance.app_version.clone(),
        })
    }

    pub(super) fn origin(&self) -> Option<String> {
        match self {
            Self::Desktop { server, .. } => server.as_ref().map(|server| server.url().into()),
            _ => None,
        }
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

    pub(super) fn version(&self) -> Option<String> {
        match self {
            Self::Android(farm) => Some(farm.device(Actor::Alice).version.version_name.clone()),
            Self::Desktop { app_version, .. } => app_version.clone(),
        }
    }

    pub(super) async fn tear_down(self, runner: &dyn CommandRunner) -> Vec<TeardownItem> {
        match self {
            Self::Android(farm) => farm.tear_down(runner).await,
            Self::Desktop {
                sessions,
                server,
                bundle: _bundle,
                ..
            } => {
                let mut items = Vec::new();
                for (actor, session) in sessions {
                    let result = end_session(session.as_ref()).await;
                    items.push(TeardownItem {
                        resource: format!("{actor} desktop"),
                        ok: result.is_ok(),
                        error: result.err().map(|e| e.to_string()),
                    });
                }
                if let Some(mut server) = server {
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

async fn end_session(session: &dyn UiSession) -> Result<(), String> {
    if let Err(first) = session.end().await {
        tracing::warn!(reason = %mortsom_common::Redactor::default().redact(&first.to_string()), "retrying owned desktop shutdown");
        session
            .end()
            .await
            .map_err(|last| format!("first shutdown: {first}; retry: {last}"))?;
    }
    Ok(())
}

#[cfg(test)]
#[path = "farm_tests.rs"]
mod tests;
