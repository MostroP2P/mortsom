//! Startup and teardown keep ownership until every child has stopped.
use super::{Runtime, error};
use mortsom_android_driver::{appium::AppiumError, process::ChildProcess};
use std::{path::Path, time::Duration};

pub(super) async fn driver_port(driver: &dyn ChildProcess, log: &Path) -> Result<u16, AppiumError> {
    tokio::time::timeout(Duration::from_secs(20), async {
        let mut tick = tokio::time::interval(Duration::from_millis(100));
        loop {
            tick.tick().await;
            if !driver.is_alive().await {
                return Err(error("ChromeDriver exited before readiness"));
            }
            let output = std::fs::read_to_string(log).map_err(error)?;
            if let Some(port) = output.lines().find_map(|line| {
                line.strip_prefix("ChromeDriver was started successfully on port ")
                    .and_then(|port| port.strip_suffix('.'))
                    .and_then(|port| port.parse::<u16>().ok())
                    .filter(|port| *port != 0)
            }) {
                return Ok(port);
            }
        }
    })
    .await
    .map_err(|_| error("ChromeDriver readiness deadline"))?
}

pub(super) async fn stop(runtime: &mut Runtime) -> Result<(), AppiumError> {
    let mut failures = Vec::new();
    if let Some(session) = &runtime.session {
        match session.delete().await {
            Ok(()) => runtime.session = None,
            Err(cause) => failures.push(cause.to_string()),
        }
    }
    if let Some(driver) = &runtime.driver {
        match driver.kill().await {
            Ok(()) => {
                runtime.driver = None;
                runtime.session = None;
            }
            Err(cause) => failures.push(cause.to_string()),
        }
    }
    if runtime.driver.is_none()
        && let Err(cause) = std::fs::remove_dir_all(&runtime.root)
        && cause.kind() != std::io::ErrorKind::NotFound
    {
        failures.push(cause.to_string());
    }
    if failures.is_empty() {
        if let Some(active) = &runtime.active {
            active.stopped().map_err(error)?;
        }
        runtime.active = None;
        Ok(())
    } else {
        Err(error(failures.join("; ")))
    }
}
