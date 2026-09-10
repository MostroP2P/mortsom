//! Isolated Linux app, desktop, credential service and accessibility session.
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use atspi::proxy::bus::{BusProxy, StatusProxy};
use atspi::zbus;
use mortsom_ui_driver::error::UiError;
use mortsom_ui_driver::process::{ChildProcess, Command, CommandRunner, run_ok};
use mortsom_ui_driver::ui::UiSession;
use tokio::sync::Mutex;

use crate::native::{Native, Node, error, label};

struct Runtime {
    active: Option<crate::storage::ActiveRuntime>,
    root: PathBuf,
    env: std::collections::BTreeMap<String, String>,
    services: Vec<Box<dyn ChildProcess>>,
    app: Option<Box<dyn ChildProcess>>,
    native: Option<Native>,
}

impl Runtime {
    fn native(&self) -> Result<&Native, UiError> {
        self.native
            .as_ref()
            .ok_or_else(|| error("accessibility session not ready"))
    }
}

/// Native app session. Each actor receives an independent runtime directory.
pub struct LinuxSession {
    runner: Arc<dyn CommandRunner>,
    executable: PathBuf,
    xvfb: PathBuf,
    storage: Option<crate::storage::ActorStorage>,
    runtime: Mutex<Option<Runtime>>,
}

impl LinuxSession {
    /// Creates an unlaunched session; `clear_app` establishes fresh storage.
    pub fn new(runner: Arc<dyn CommandRunner>, executable: PathBuf, xvfb: PathBuf) -> Self {
        Self {
            runner,
            executable,
            xvfb,
            storage: None,
            runtime: Mutex::new(None),
        }
    }

    /// Retains caller-owned actor data across process shutdown and recovery.
    pub fn with_storage(mut self, storage: crate::storage::ActorStorage) -> Self {
        self.storage = Some(storage);
        self
    }

    async fn prepare(&self, slot: &mut Option<Runtime>) -> Result<(), UiError> {
        use std::os::unix::fs::DirBuilderExt;
        let storage = self
            .storage
            .as_ref()
            .map(|storage| storage.prepare(crate::Platform::Linux))
            .transpose()
            .map_err(error)?;
        let root = std::env::temp_dir().join(format!("mortsom-{}", mortsom_common::RunId::new()));
        std::fs::DirBuilder::new()
            .mode(0o700)
            .create(&root)
            .map_err(error)?;
        *slot = Some(Runtime {
            active: None,
            root,
            env: Default::default(),
            native: None,
            services: Vec::new(),
            app: None,
        });
        let runtime = slot
            .as_mut()
            .ok_or_else(|| error("missing startup ownership"))?;
        runtime.active = storage
            .as_deref()
            .map(crate::storage::ActiveRuntime::begin)
            .transpose()
            .map_err(error)?;
        match self
            .prepare_services(
                &runtime.root,
                storage.as_deref().unwrap_or(&runtime.root),
                &mut runtime.services,
            )
            .await
        {
            Ok((env, native)) => {
                runtime.env = env;
                runtime.native = Some(native);
                Ok(())
            }
            Err(cause) => match stop(runtime).await {
                Ok(()) => {
                    *slot = None;
                    Err(cause)
                }
                Err(cleanup) => Err(error(format!("{cause}; startup cleanup failed: {cleanup}"))),
            },
        }
    }

    async fn prepare_services(
        &self,
        root: &Path,
        storage: &Path,
        services: &mut Vec<Box<dyn ChildProcess>>,
    ) -> Result<(std::collections::BTreeMap<String, String>, Native), UiError> {
        let display_file = root.join("display");
        services.push(
            self.runner
                .spawn(
                    &Command::new(
                        &self.xvfb,
                        &[
                            "-displayfd",
                            "1",
                            "-screen",
                            "0",
                            "1280x960x24",
                            "-nolisten",
                            "tcp",
                            "-ac",
                        ],
                    ),
                    Some(&display_file),
                )
                .await?,
        );
        let display = tokio::time::timeout(Duration::from_secs(15), async {
            let mut tick = tokio::time::interval(Duration::from_millis(100));
            loop {
                tick.tick().await;
                if !services[0].is_alive().await {
                    return Err(error("Xvfb exited before display readiness"));
                }
                let value = std::fs::read_to_string(&display_file).map_err(error)?;
                if let Some(number) = value
                    .lines()
                    .find(|line| !line.is_empty() && line.chars().all(|c| c.is_ascii_digit()))
                {
                    return Ok::<_, UiError>(format!(":{number}"));
                }
            }
        })
        .await
        .map_err(error)??;
        let address = format!("unix:path={}/bus", root.display());
        let env = std::collections::BTreeMap::from([
            ("DBUS_SESSION_BUS_ADDRESS".into(), address.clone()),
            ("DISPLAY".into(), display),
            (
                "XDG_DATA_HOME".into(),
                storage.join("data").display().to_string(),
            ),
            (
                "XDG_CONFIG_HOME".into(),
                storage.join("config").display().to_string(),
            ),
            (
                "XDG_CACHE_HOME".into(),
                root.join("cache").display().to_string(),
            ),
            ("XDG_RUNTIME_DIR".into(), root.display().to_string()),
            ("GDK_BACKEND".into(), "x11".into()),
            ("NO_AT_BRIDGE".into(), "0".into()),
            ("GSETTINGS_BACKEND".into(), "memory".into()),
            ("GIO_USE_VFS".into(), "local".into()),
            ("GTK_USE_PORTAL".into(), "0".into()),
            ("ATSPI_DBUS_IMPLEMENTATION".into(), "dbus-daemon".into()),
        ]);
        let bus_config = root.join("bus.conf");
        std::fs::write(&bus_config, format!(r#"<busconfig><type>session</type><listen>{address}</listen><auth>EXTERNAL</auth><policy context="default"><allow send_destination="*"/><allow receive_sender="*"/><allow own="*"/></policy></busconfig>"#)).map_err(error)?;
        let mut bus = Command::new(
            "dbus-daemon",
            &[
                "--nofork",
                &format!("--config-file={}", bus_config.display()),
            ],
        );
        bus.env = env.clone();
        services.push(self.runner.spawn(&bus, None).await?);
        let connection = connect_bus(&address).await?;
        let mut accessibility = Command::new(
            "/usr/libexec/at-spi-bus-launcher",
            &["--launch-immediately"],
        );
        accessibility.env = env.clone();
        services.push(self.runner.spawn(&accessibility, None).await?);
        let bus_proxy = zbus::fdo::DBusProxy::new(&connection)
            .await
            .map_err(error)?;
        tokio::time::timeout(Duration::from_secs(15), async {
            let mut tick = tokio::time::interval(Duration::from_millis(100));
            loop {
                tick.tick().await;
                if bus_proxy
                    .name_has_owner("org.a11y.Bus".try_into().map_err(error)?)
                    .await
                    .map_err(error)?
                {
                    return Ok::<_, UiError>(());
                }
            }
        })
        .await
        .map_err(error)??;
        let status = StatusProxy::new(&connection)
            .await
            .map_err(|e| error(format!("status proxy: {e}")))?;
        status
            .set_is_enabled(true)
            .await
            .map_err(|e| error(format!("enable accessibility: {e}")))?;
        status
            .set_screen_reader_enabled(true)
            .await
            .map_err(|e| error(format!("enable screen reader: {e}")))?;
        let a11y = BusProxy::new(&connection)
            .await
            .map_err(error)?
            .get_address()
            .await
            .map_err(error)?;
        let mut env = env;
        env.insert("AT_SPI_BUS_ADDRESS".into(), a11y.clone());
        let mut registry = Command::new("/usr/libexec/at-spi2-registryd", &[]);
        registry.env = env.clone();
        services.push(self.runner.spawn(&registry, None).await?);
        let mut keyring = Command::new(
            "sh",
            &[
                "-c",
                "printf '\\n' | exec gnome-keyring-daemon --foreground --unlock --components=secrets --control-directory=\"$1\"",
                "mortsom-keyring",
                &root.join("keyring").display().to_string(),
            ],
        );
        keyring.env = env.clone();
        services.push(self.runner.spawn(&keyring, None).await?);
        wait_for_keyring(&connection).await?;
        Ok((env, Native::connect(&a11y).await?))
    }

    async fn find(&self, id: &str) -> Result<Node, UiError> {
        self.runtime
            .lock()
            .await
            .as_ref()
            .ok_or_else(|| error("session not prepared"))?
            .native()?
            .find(id)
            .await?
            .ok_or_else(|| UiError::WaitTimeout {
                accessibility_id: id.into(),
                condition: "not found",
                timeout: Duration::ZERO,
            })
    }

    async fn wait(&self, id: &str, present: bool, timeout: Duration) -> Result<(), UiError> {
        let result = tokio::time::timeout(timeout, async {
            let mut tick = tokio::time::interval(Duration::from_millis(150));
            loop {
                tick.tick().await;
                if self.exists(id).await? == present {
                    return Ok(());
                }
            }
        })
        .await;
        result.unwrap_or_else(|_| {
            Err(UiError::WaitTimeout {
                accessibility_id: id.into(),
                condition: if present {
                    "not found"
                } else {
                    "still present"
                },
                timeout,
            })
        })
    }
}

async fn wait_for_keyring(connection: &zbus::Connection) -> Result<(), UiError> {
    tokio::time::timeout(Duration::from_secs(15), async {
        let bus = zbus::fdo::DBusProxy::new(connection).await.map_err(error)?;
        let mut tick = tokio::time::interval(Duration::from_millis(100));
        loop {
            tick.tick().await;
            if !bus
                .name_has_owner("org.freedesktop.secrets".try_into().map_err(error)?)
                .await
                .map_err(error)?
            {
                continue;
            }
            let service = zbus::Proxy::new(
                connection,
                "org.freedesktop.secrets",
                "/org/freedesktop/secrets",
                "org.freedesktop.Secret.Service",
            )
            .await
            .map_err(error)?;
            let collection: zbus::zvariant::OwnedObjectPath = service
                .call("ReadAlias", &("default",))
                .await
                .map_err(error)?;
            if collection.as_str() == "/" {
                continue;
            }
            let collection = zbus::Proxy::new(
                connection,
                "org.freedesktop.secrets",
                collection,
                "org.freedesktop.Secret.Collection",
            )
            .await
            .map_err(error)?;
            if !collection
                .get_property::<bool>("Locked")
                .await
                .map_err(error)?
            {
                return Ok::<_, UiError>(());
            }
        }
    })
    .await
    .map_err(error)?
}

async fn connect_bus(address: &str) -> Result<zbus::Connection, UiError> {
    tokio::time::timeout(Duration::from_secs(15), async {
        let mut tick = tokio::time::interval(Duration::from_millis(100));
        loop {
            tick.tick().await;
            if let Ok(connection) = zbus::connection::Builder::address(address)
                .map_err(error)?
                .build()
                .await
            {
                return Ok(connection);
            }
        }
    })
    .await
    .map_err(error)?
}

async fn stop(runtime: &mut Runtime) -> Result<(), UiError> {
    let mut failures = Vec::new();
    if let Some(app) = &runtime.app {
        match app.kill().await {
            Ok(()) => runtime.app = None,
            Err(cause) => failures.push(cause.to_string()),
        }
    }
    for index in (0..runtime.services.len()).rev() {
        match runtime.services[index].kill().await {
            Ok(()) => {
                runtime.services.remove(index);
            }
            Err(cause) => failures.push(cause.to_string()),
        }
    }
    if !failures.is_empty() {
        return Err(error(failures.join("; ")));
    }
    runtime.native = None;
    match std::fs::remove_dir_all(&runtime.root) {
        Ok(()) => {}
        Err(cause) if cause.kind() == std::io::ErrorKind::NotFound => {}
        Err(cause) => return Err(error(cause)),
    }
    if let Some(active) = &runtime.active {
        active.stopped().map_err(error)?;
    }
    runtime.active = None;
    Ok(())
}

#[async_trait]
impl UiSession for LinuxSession {
    async fn exists(&self, id: &str) -> Result<bool, UiError> {
        let guard = self.runtime.lock().await;
        Ok(guard
            .as_ref()
            .ok_or_else(|| error("session not prepared"))?
            .native()?
            .find(id)
            .await?
            .is_some())
    }
    async fn wait_for(&self, id: &str, timeout: Duration) -> Result<(), UiError> {
        self.wait(id, true, timeout).await
    }
    async fn wait_gone(&self, id: &str, timeout: Duration) -> Result<(), UiError> {
        self.wait(id, false, timeout).await
    }
    async fn tap(&self, id: &str) -> Result<(), UiError> {
        let node = self.find(id).await?;
        self.runtime
            .lock()
            .await
            .as_ref()
            .ok_or_else(|| error("session not prepared"))?
            .native()?
            .click(&node)
            .await
    }
    async fn type_text(&self, id: &str, text: &str) -> Result<(), UiError> {
        let node = self.find(id).await?;
        self.runtime
            .lock()
            .await
            .as_ref()
            .ok_or_else(|| error("session not prepared"))?
            .native()?
            .type_text(&node, text)
            .await
    }
    async fn label_of(&self, id: &str) -> Result<String, UiError> {
        Ok(label(&self.find(id).await?.name)
            .map(|(_, value)| value.to_owned())
            .unwrap_or_default())
    }
    async fn text_of(&self, id: &str) -> Result<String, UiError> {
        let node = self.find(id).await?;
        self.runtime
            .lock()
            .await
            .as_ref()
            .ok_or_else(|| error("session not prepared"))?
            .native()?
            .text_value(&node)
            .await
    }
    async fn is_enabled(&self, id: &str) -> Result<bool, UiError> {
        let node = self.find(id).await?;
        self.runtime
            .lock()
            .await
            .as_ref()
            .ok_or_else(|| error("session not prepared"))?
            .native()?
            .enabled(&node)
            .await
    }
    async fn screenshot_png(&self) -> Result<Vec<u8>, UiError> {
        let guard = self.runtime.lock().await;
        let runtime = guard
            .as_ref()
            .ok_or_else(|| error("session not prepared"))?;
        let path = runtime.root.join("screen.png");
        let mut cmd = Command::new("import", &["-window", "root", &path.display().to_string()]);
        cmd.env = runtime.env.clone();
        run_ok(self.runner.as_ref(), &cmd, Duration::from_secs(15)).await?;
        let image = std::fs::read(&path).map_err(error)?;
        std::fs::remove_file(path).map_err(error)?;
        Ok(image)
    }
    async fn page_source(&self) -> Result<String, UiError> {
        let guard = self.runtime.lock().await;
        let runtime = guard
            .as_ref()
            .ok_or_else(|| error("session not prepared"))?;
        let mut xml = String::from("<hierarchy>");
        for node in runtime.native()?.nodes().await? {
            if let Some((id, value)) = label(&node.name) {
                xml.push_str(&format!(
                    "<node resource-id=\"{}\" content-desc=\"{}\" focused=\"{}\"/>",
                    crate::xml_escape(id),
                    crate::xml_escape(value),
                    runtime.native()?.focused(&node).await?
                ));
            }
        }
        xml.push_str("</hierarchy>");
        Ok(xml)
    }
    async fn background(&self, _: u64) -> Result<(), UiError> {
        Err(error("Linux background capability not implemented"))
    }
    async fn terminate_app(&self) -> Result<(), UiError> {
        if let Some(runtime) = self.runtime.lock().await.as_mut()
            && let Some(app) = &runtime.app
        {
            app.kill().await?;
            runtime.app = None;
        }
        Ok(())
    }
    async fn activate_app(&self) -> Result<(), UiError> {
        let mut guard = self.runtime.lock().await;
        if guard.is_none()
            && self
                .storage
                .as_ref()
                .is_some_and(|storage| storage.is_resume())
        {
            self.prepare(&mut guard).await?;
        }
        let runtime = guard
            .as_mut()
            .ok_or_else(|| error("session not prepared"))?;
        runtime.native()?;
        for service in &runtime.services {
            if !service.is_alive().await {
                return Err(error("owned desktop service exited before app launch"));
            }
        }
        if let Some(app) = &runtime.app {
            if app.is_alive().await {
                return Ok(());
            }
            app.kill().await?;
            runtime.app = None;
        }
        let mut cmd = Command::new(&self.executable, &[]);
        cmd.env = runtime.env.clone();
        runtime.app = Some(
            self.runner
                .spawn(&cmd, Some(&runtime.root.join("app.log")))
                .await?,
        );
        Ok(())
    }
    async fn clear_app(&self) -> Result<(), UiError> {
        if self
            .storage
            .as_ref()
            .is_some_and(|storage| storage.is_resume())
        {
            return Err(error("cannot clear protected recovery storage"));
        }
        let mut guard = self.runtime.lock().await;
        if let Some(runtime) = guard.as_mut() {
            stop(runtime).await?;
            *guard = None;
        }
        self.prepare(&mut guard).await
    }
    async fn set_connectivity(&self, _: bool) -> Result<(), UiError> {
        Err(error("Linux network capability not implemented"))
    }
    async fn set_orientation(&self, _: bool) -> Result<(), UiError> {
        Err(error("Linux rotation capability not implemented"))
    }
    async fn hide_keyboard(&self) -> Result<(), UiError> {
        Ok(())
    }
    /// The app's stdout and stderr, which the Rust core's bridge log and
    /// Flutter's own prints go to: the only record of what the client did
    /// between two screens.
    async fn diagnostics(&self) -> Vec<(String, String)> {
        let guard = self.runtime.lock().await;
        let Some(runtime) = guard.as_ref() else {
            return Vec::new();
        };
        std::fs::read_to_string(runtime.root.join("app.log"))
            .map(|text| vec![("app.log".to_owned(), text)])
            .unwrap_or_default()
    }
    async fn end(&self) -> Result<(), UiError> {
        let mut guard = self.runtime.lock().await;
        if let Some(runtime) = guard.as_mut() {
            stop(runtime).await?;
            *guard = None;
        }
        Ok(())
    }
}

#[cfg(test)]
#[path = "linux_lifecycle_tests.rs"]
mod lifecycle_tests;
