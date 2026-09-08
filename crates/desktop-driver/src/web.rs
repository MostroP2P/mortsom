//! W3C WebDriver automation of Flutter's public semantics DOM.
use async_trait::async_trait;
use mortsom_android_driver::{
    appium::AppiumError,
    process::{ChildProcess, Command, CommandRunner},
    ui::UiSession,
    webdriver::{Element, Locator, Session},
};
use serde_json::json;
use std::{path::PathBuf, sync::Arc, time::Duration};
use tokio::sync::Mutex;

mod lifecycle;
use lifecycle::{driver_port, stop};

pub(crate) fn error(cause: impl std::fmt::Display) -> AppiumError {
    AppiumError::WebDriver(
        mortsom_android_driver::webdriver::WebDriverError::Transport(
            mortsom_common::Redactor::default().redact(&format!("Web automation: {cause}")),
        ),
    )
}

struct Runtime {
    session: Option<Session>,
    driver: Option<Box<dyn ChildProcess>>,
    root: PathBuf,
}

/// Each actor owns a browser process and an independent browser profile.
pub struct WebSession {
    runner: Arc<dyn CommandRunner>,
    url: String,
    driver: PathBuf,
    browser: PathBuf,
    runtime: Mutex<Option<Runtime>>,
}

impl WebSession {
    /// Creates a browser session descriptor. `clear_app` allocates a clean profile.
    pub fn new(
        runner: Arc<dyn CommandRunner>,
        url: String,
        driver: PathBuf,
        browser: PathBuf,
    ) -> Self {
        Self {
            runner,
            url,
            driver,
            browser,
            runtime: Mutex::new(None),
        }
    }

    async fn session(&self) -> Result<Session, AppiumError> {
        self.runtime
            .lock()
            .await
            .as_ref()
            .and_then(|r| r.session.clone())
            .ok_or_else(|| error("browser session is not ready"))
    }
    async fn element(&self, id: &str) -> Result<Element, AppiumError> {
        Ok(self
            .session()
            .await?
            .find_element(Locator::XPath, &selector(id)?)
            .await?)
    }
    async fn wait(&self, id: &str, present: bool, timeout: Duration) -> Result<(), AppiumError> {
        tokio::time::timeout(timeout, async {
            let mut tick = tokio::time::interval(Duration::from_millis(150));
            loop {
                tick.tick().await;
                if self.exists(id).await? == present {
                    return Ok(());
                }
            }
        })
        .await
        .unwrap_or_else(|_| {
            Err(AppiumError::WaitTimeout {
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
    async fn create(&self, slot: &mut Option<Runtime>) -> Result<(), AppiumError> {
        use std::os::unix::fs::DirBuilderExt;
        let root =
            std::env::temp_dir().join(format!("mortsom-browser-{}", mortsom_common::RunId::new()));
        std::fs::DirBuilder::new()
            .mode(0o700)
            .create(&root)
            .map_err(error)?;
        *slot = Some(Runtime {
            session: None,
            driver: None,
            root,
        });
        let runtime = slot
            .as_mut()
            .ok_or_else(|| error("missing startup ownership"))?;
        if let Err(cause) = self.start(runtime).await {
            return match stop(runtime).await {
                Ok(()) => {
                    *slot = None;
                    Err(cause)
                }
                Err(cleanup) => Err(error(format!("{cause}; startup cleanup failed: {cleanup}"))),
            };
        }
        Ok(())
    }

    async fn start(&self, runtime: &mut Runtime) -> Result<(), AppiumError> {
        let log = runtime.root.join("driver.log");
        runtime.driver = Some(
            self.runner
                .spawn(
                    &Command::new(&self.driver, &["--port=0", "--allowed-ips=127.0.0.1"]),
                    Some(&log),
                )
                .await?,
        );
        let driver = runtime
            .driver
            .as_ref()
            .ok_or_else(|| error("driver not started"))?;
        let port = driver_port(driver.as_ref(), &log).await?;
        let endpoint = format!("http://127.0.0.1:{port}");
        let capabilities = json!({"browserName":"chrome", "goog:chromeOptions": {
            "binary": self.browser, "args": ["--headless=new", "--window-size=1280,960", "--disable-background-networking", "--no-first-run", format!("--user-data-dir={}", runtime.root.join("profile").display())]
        }});
        runtime.session =
            Some(Session::create(&endpoint, capabilities, Duration::from_secs(30)).await?);
        runtime
            .session
            .as_ref()
            .ok_or_else(|| error("session not created"))?
            .post("url", &json!({"url":self.url}))
            .await?;
        Ok(())
    }
}

/// Exact identifier selector. IDs are contract values, not arbitrary XPath.
pub fn selector(id: &str) -> Result<String, AppiumError> {
    if id.is_empty() || id.chars().any(char::is_control) {
        return Err(error("invalid automation identifier"));
    }
    let literal = if id.contains('\'') {
        format!(
            "concat({})",
            id.split('\'')
                .map(|part| format!("'{part}'"))
                .collect::<Vec<_>>()
                .join(",\"'\",")
        )
    } else {
        format!("'{id}'")
    };
    Ok(format!("//*[@flt-semantics-identifier={literal}]"))
}

#[async_trait]
impl UiSession for WebSession {
    async fn exists(&self, id: &str) -> Result<bool, AppiumError> {
        Ok(!self
            .session()
            .await?
            .find_elements(Locator::XPath, &selector(id)?)
            .await?
            .is_empty())
    }
    async fn wait_for(&self, id: &str, timeout: Duration) -> Result<(), AppiumError> {
        self.wait(id, true, timeout).await
    }
    async fn wait_gone(&self, id: &str, timeout: Duration) -> Result<(), AppiumError> {
        self.wait(id, false, timeout).await
    }
    async fn tap(&self, id: &str) -> Result<(), AppiumError> {
        Ok(self
            .session()
            .await?
            .click(&self.element(id).await?)
            .await?)
    }
    async fn type_text(&self, id: &str, text: &str) -> Result<(), AppiumError> {
        crate::web_input::enter(&self.session().await?, id, text, Duration::from_secs(5)).await
    }
    async fn text_of(&self, id: &str) -> Result<String, AppiumError> {
        let session = self.session().await?;
        match crate::web_input::read(&session, id).await {
            Ok(value) => Ok(value),
            Err(AppiumError::WebDriver(cause)) if cause.is_no_such_element() => {
                Ok(session.text(&self.element(id).await?).await?)
            }
            Err(cause) => Err(cause),
        }
    }
    async fn label_of(&self, id: &str) -> Result<String, AppiumError> {
        let session = self.session().await?;
        let node = self.element(id).await?;
        let label = session
            .get(&format!("element/{}/attribute/aria-label", node.id))
            .await?;
        match label.as_str() {
            Some(value) => Ok(value.to_owned()),
            None => Ok(session.text(&node).await?.trim().to_owned()),
        }
    }
    async fn is_enabled(&self, id: &str) -> Result<bool, AppiumError> {
        let session = self.session().await?;
        let node = self.element(id).await?;
        let disabled = session
            .get(&format!("element/{}/attribute/aria-disabled", node.id))
            .await?;
        Ok(disabled.as_str() != Some("true") && session.is_enabled(&node).await?)
    }
    async fn screenshot_png(&self) -> Result<Vec<u8>, AppiumError> {
        Ok(self.session().await?.screenshot_png().await?)
    }
    async fn page_source(&self) -> Result<String, AppiumError> {
        Ok(self
            .session()
            .await?
            .source()
            .await?
            .replace("flt-semantics-identifier=", "resource-id="))
    }
    async fn background(&self, _: u64) -> Result<(), AppiumError> {
        Err(error("Web background capability is not implemented"))
    }
    async fn terminate_app(&self) -> Result<(), AppiumError> {
        self.session()
            .await?
            .post("url", &json!({"url":"about:blank"}))
            .await?;
        Ok(())
    }
    async fn activate_app(&self) -> Result<(), AppiumError> {
        self.session()
            .await?
            .post("url", &json!({"url":self.url}))
            .await?;
        Ok(())
    }
    async fn clear_app(&self) -> Result<(), AppiumError> {
        self.end().await?;
        self.create(&mut *self.runtime.lock().await).await
    }
    async fn set_connectivity(&self, _: bool) -> Result<(), AppiumError> {
        Err(error("Web network capability is not implemented"))
    }
    async fn set_orientation(&self, _: bool) -> Result<(), AppiumError> {
        Err(error("Web rotation capability is not implemented"))
    }
    async fn hide_keyboard(&self) -> Result<(), AppiumError> {
        Ok(())
    }
    async fn end(&self) -> Result<(), AppiumError> {
        let mut slot = self.runtime.lock().await;
        if let Some(runtime) = slot.as_mut() {
            stop(runtime).await?;
            *slot = None;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn xpath_uses_the_exact_public_identifier_and_rejects_injection() {
        assert!(selector("order.status").is_ok());
        assert!(selector("order.book.item.a12-b34").is_ok());
        assert_eq!(
            selector("x'] | //input").unwrap_or_default(),
            "//*[@flt-semantics-identifier=concat('x',\"'\",'] | //input')]"
        );
        assert!(selector("settings.relays.item.ws://127.0.0.1:7000").is_ok());
    }
}

#[cfg(test)]
#[path = "web_lifecycle_tests.rs"]
mod lifecycle_tests;
