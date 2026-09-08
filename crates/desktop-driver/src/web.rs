//! W3C WebDriver automation of Flutter's public semantics DOM.
use crate::native::error;
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

struct Runtime {
    session: Session,
    driver: Box<dyn ChildProcess>,
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
            .map(|r| r.session.clone())
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
    async fn create(&self) -> Result<Runtime, AppiumError> {
        use std::os::unix::fs::DirBuilderExt;
        let root =
            std::env::temp_dir().join(format!("mortsom-browser-{}", mortsom_common::RunId::new()));
        std::fs::DirBuilder::new()
            .mode(0o700)
            .create(&root)
            .map_err(error)?;
        let port = std::net::TcpListener::bind("127.0.0.1:0")
            .map_err(error)?
            .local_addr()
            .map_err(error)?
            .port();
        let driver = self
            .runner
            .spawn(
                &Command::new(
                    &self.driver,
                    &[&format!("--port={port}"), "--allowed-ips=127.0.0.1"],
                ),
                None,
            )
            .await?;
        let result = self.connect(port, &root).await;
        match result {
            Ok(session) => Ok(Runtime {
                session,
                driver,
                root,
            }),
            Err(cause) => {
                let shutdown = driver.kill().await;
                let cleanup = std::fs::remove_dir_all(root);
                shutdown?;
                cleanup.map_err(error)?;
                Err(cause)
            }
        }
    }
    async fn connect(&self, port: u16, root: &std::path::Path) -> Result<Session, AppiumError> {
        let endpoint = format!("http://127.0.0.1:{port}");
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(2))
            .build()
            .map_err(error)?;
        tokio::time::timeout(Duration::from_secs(20), async {
            let mut tick = tokio::time::interval(Duration::from_millis(100));
            loop {
                tick.tick().await;
                if http
                    .get(format!("{endpoint}/status"))
                    .send()
                    .await
                    .is_ok_and(|r| r.status().is_success())
                {
                    return;
                }
            }
        })
        .await
        .map_err(error)?;
        let capabilities = json!({"browserName":"chrome", "goog:chromeOptions": {
            "binary": self.browser, "args": ["--headless=new", "--window-size=1280,960", "--disable-background-networking", "--no-first-run", format!("--user-data-dir={}", root.display())]
        }});
        let session = Session::create(&endpoint, capabilities, Duration::from_secs(30)).await?;
        if let Err(cause) = session.post("url", &json!({"url":self.url})).await {
            session.delete().await?;
            return Err(cause.into());
        }
        Ok(session)
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
        let session = self.session().await?;
        let selector = selector(id)?;
        let input = session
            .find_element(
                Locator::XPath,
                &format!("{selector}/descendant-or-self::*[self::input or self::textarea]"),
            )
            .await?;
        // Flutter keeps off-screen semantics nodes in the DOM. A user click
        // scrolls the field into view before WebDriver can edit it.
        session.click(&input).await?;
        session.clear(&input).await?;
        Ok(session.send_keys(&input, text).await?)
    }
    async fn text_of(&self, id: &str) -> Result<String, AppiumError> {
        Ok(self.session().await?.text(&self.element(id).await?).await?)
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
        *self.runtime.lock().await = Some(self.create().await?);
        Ok(())
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
        if let Some(runtime) = self.runtime.lock().await.take() {
            let session = runtime.session.delete().await;
            let driver = runtime.driver.kill().await;
            let files = std::fs::remove_dir_all(runtime.root);
            session?;
            driver?;
            files.map_err(error)?;
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
