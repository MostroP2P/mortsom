//! Thin W3C WebDriver client for ChromeDriver (spec §6.2). Only the
//! endpoints Mortsom needs; every response follows the W3C `{"value": ...}`
//! envelope and errors surface as [`WebDriverError`].

use std::time::Duration;

use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use thiserror::Error;

/// Locator strategies Mortsom uses. Accessibility ids are the contract
/// (spec §3); the others exist for native dialogs and diagnostics only.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Locator {
    /// `accessibility id` — the app's stable semantic identifier.
    AccessibilityId,
    /// `id` — a native resource id.
    ResourceId,
    /// `xpath` — the Web session's semantics selectors.
    XPath,
}

impl Locator {
    /// The `using` value on the wire.
    pub fn using(self) -> &'static str {
        match self {
            Locator::AccessibilityId => "accessibility id",
            Locator::ResourceId => "id",
            Locator::XPath => "xpath",
        }
    }
}

/// WebDriver failures.
#[derive(Debug, Error)]
pub enum WebDriverError {
    /// Transport failure.
    #[error("webdriver transport error: {0}")]
    Transport(String),
    /// The server returned a W3C error object.
    #[error("webdriver {error}: {message}")]
    Server {
        /// W3C error code, e.g. `no such element`.
        error: String,
        /// Server message.
        message: String,
    },
    /// The response did not have the expected shape.
    #[error("unexpected webdriver response: {0}")]
    Shape(String),
}

impl WebDriverError {
    /// True for `no such element` errors, which drive waits.
    pub fn is_no_such_element(&self) -> bool {
        matches!(self, WebDriverError::Server { error, .. } if error == "no such element")
    }

    /// True for a request that never got an answer.
    ///
    /// A transport failure says nothing about the app: the screen was never
    /// read. Inside a bounded wait it is a reason to poll again, not the
    /// wait's verdict.
    pub fn is_transport(&self) -> bool {
        matches!(self, WebDriverError::Transport(_))
    }
}

/// A raw WebDriver session.
#[derive(Debug, Clone)]
pub struct Session {
    http: reqwest::Client,
    base: String,
    id: String,
}

/// W3C element reference key.
const ELEMENT_KEY: &str = "element-6066-11e4-a52e-4f735466cecf";

/// An element handle within a session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Element {
    /// Element reference id.
    pub id: String,
}

impl Session {
    /// Creates a session at `base` (e.g. `http://127.0.0.1:4723`) with the
    /// given `alwaysMatch` capabilities.
    pub async fn create(
        base: &str,
        capabilities: Value,
        timeout: Duration,
    ) -> Result<Self, WebDriverError> {
        let http = reqwest::Client::builder()
            .timeout(timeout)
            .build()
            .map_err(|e| WebDriverError::Transport(e.to_string()))?;
        let base = base.trim_end_matches('/').to_owned();
        let body = json!({ "capabilities": { "alwaysMatch": capabilities } });
        let value = post_json(&http, &format!("{base}/session"), &body).await?;
        let id = value
            .get("sessionId")
            .and_then(Value::as_str)
            .ok_or_else(|| WebDriverError::Shape("missing sessionId".into()))?
            .to_owned();
        Ok(Self { http, base, id })
    }

    /// Reattaches to an existing session id (used by tests and recovery).
    pub fn attach(base: &str, id: &str, timeout: Duration) -> Result<Self, WebDriverError> {
        let http = reqwest::Client::builder()
            .timeout(timeout)
            .build()
            .map_err(|e| WebDriverError::Transport(e.to_string()))?;
        Ok(Self {
            http,
            base: base.trim_end_matches('/').to_owned(),
            id: id.to_owned(),
        })
    }

    /// Session id.
    pub fn id(&self) -> &str {
        &self.id
    }

    fn url(&self, path: &str) -> String {
        format!(
            "{}/session/{}/{}",
            self.base,
            self.id,
            path.trim_start_matches('/')
        )
    }

    /// POSTs `body` to a session-relative path and returns the `value`.
    pub async fn post(&self, path: &str, body: &impl Serialize) -> Result<Value, WebDriverError> {
        post_json(&self.http, &self.url(path), body).await
    }

    /// GETs a session-relative path and returns the `value`.
    pub async fn get(&self, path: &str) -> Result<Value, WebDriverError> {
        let response = self
            .http
            .get(self.url(path))
            .send()
            .await
            .map_err(|e| WebDriverError::Transport(e.to_string()))?;
        unwrap_value(response).await
    }

    /// Finds one element.
    pub async fn find_element(
        &self,
        locator: Locator,
        value: &str,
    ) -> Result<Element, WebDriverError> {
        let body = json!({ "using": locator.using(), "value": value });
        let found = self.post("element", &body).await?;
        element_from(&found)
    }

    /// Finds every matching element.
    pub async fn find_elements(
        &self,
        locator: Locator,
        value: &str,
    ) -> Result<Vec<Element>, WebDriverError> {
        let body = json!({ "using": locator.using(), "value": value });
        let found = self.post("elements", &body).await?;
        found
            .as_array()
            .ok_or_else(|| WebDriverError::Shape("elements is not an array".into()))?
            .iter()
            .map(element_from)
            .collect()
    }

    /// Clicks an element.
    pub async fn click(&self, element: &Element) -> Result<(), WebDriverError> {
        self.post(&format!("element/{}/click", element.id), &json!({}))
            .await?;
        Ok(())
    }

    /// Types text into an element. The text may be a secret and is never
    /// logged.
    #[tracing::instrument(skip(self, text, element))]
    pub async fn send_keys(&self, element: &Element, text: &str) -> Result<(), WebDriverError> {
        self.post(
            &format!("element/{}/value", element.id),
            &json!({ "text": text }),
        )
        .await?;
        Ok(())
    }

    /// Clears an element.
    pub async fn clear(&self, element: &Element) -> Result<(), WebDriverError> {
        self.post(&format!("element/{}/clear", element.id), &json!({}))
            .await?;
        Ok(())
    }

    /// Visible text of an element.
    pub async fn text(&self, element: &Element) -> Result<String, WebDriverError> {
        string_value(self.get(&format!("element/{}/text", element.id)).await?)
    }

    /// An element attribute (`enabled`, `content-desc`, ...).
    pub async fn attribute(&self, element: &Element, name: &str) -> Result<String, WebDriverError> {
        string_value(
            self.get(&format!("element/{}/attribute/{name}", element.id))
                .await?,
        )
    }

    /// Whether an element is enabled.
    pub async fn is_enabled(&self, element: &Element) -> Result<bool, WebDriverError> {
        self.get(&format!("element/{}/enabled", element.id))
            .await?
            .as_bool()
            .ok_or_else(|| WebDriverError::Shape("enabled is not a bool".into()))
    }

    /// Page source (the accessibility hierarchy as XML).
    pub async fn source(&self) -> Result<String, WebDriverError> {
        string_value(self.get("source").await?)
    }

    /// PNG screenshot bytes.
    pub async fn screenshot_png(&self) -> Result<Vec<u8>, WebDriverError> {
        use base64::Engine;
        let encoded = string_value(self.get("screenshot").await?)?;
        base64::engine::general_purpose::STANDARD
            .decode(encoded.trim())
            .map_err(|e| WebDriverError::Shape(format!("screenshot is not base64: {e}")))
    }

    /// Executes a synchronous script in the page.
    pub async fn execute(&self, script: &str, args: Vec<Value>) -> Result<Value, WebDriverError> {
        self.post("execute/sync", &json!({ "script": script, "args": args }))
            .await
    }

    /// Sets the implicit wait.
    pub async fn set_implicit_wait(&self, wait: Duration) -> Result<(), WebDriverError> {
        self.post("timeouts", &json!({ "implicit": wait.as_millis() as u64 }))
            .await?;
        Ok(())
    }

    /// Ends the session.
    pub async fn delete(&self) -> Result<(), WebDriverError> {
        let response = self
            .http
            .delete(format!("{}/session/{}", self.base, self.id))
            .send()
            .await
            .map_err(|e| WebDriverError::Transport(e.to_string()))?;
        unwrap_value(response).await.map(|_| ())
    }
}

/// GET `<base>/status`; true when the server answers with `ready: true`.
pub async fn server_ready(base: &str, timeout: Duration) -> bool {
    let Ok(http) = reqwest::Client::builder().timeout(timeout).build() else {
        return false;
    };
    let url = format!("{}/status", base.trim_end_matches('/'));
    match http.get(url).send().await {
        Ok(response) => unwrap_value(response)
            .await
            .map(|v| v.get("ready").and_then(Value::as_bool).unwrap_or(false))
            .unwrap_or(false),
        Err(_) => false,
    }
}

async fn post_json(
    http: &reqwest::Client,
    url: &str,
    body: &impl Serialize,
) -> Result<Value, WebDriverError> {
    let response = http
        .post(url)
        .json(body)
        .send()
        .await
        .map_err(|e| WebDriverError::Transport(e.to_string()))?;
    unwrap_value(response).await
}

async fn unwrap_value(response: reqwest::Response) -> Result<Value, WebDriverError> {
    let status = response.status();
    let body: Value = response
        .json()
        .await
        .map_err(|e| WebDriverError::Shape(format!("non-JSON body (HTTP {status}): {e}")))?;
    let value = body
        .get("value")
        .cloned()
        .ok_or_else(|| WebDriverError::Shape("missing value".into()))?;
    if let Some(error) = value.get("error").and_then(Value::as_str) {
        return Err(WebDriverError::Server {
            error: error.to_owned(),
            message: value
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_owned(),
        });
    }
    if !status.is_success() {
        return Err(WebDriverError::Server {
            error: format!("http {status}"),
            message: value.to_string(),
        });
    }
    Ok(value)
}

/// Accepts the W3C element key and, for compatibility with older drivers,
/// the legacy `ELEMENT` key.
fn element_from(value: &Value) -> Result<Element, WebDriverError> {
    value
        .get(ELEMENT_KEY)
        .or_else(|| value.get("ELEMENT"))
        .and_then(Value::as_str)
        .map(|id| Element { id: id.to_owned() })
        .ok_or_else(|| WebDriverError::Shape("missing element reference".into()))
}

fn string_value(value: Value) -> Result<String, WebDriverError> {
    value
        .as_str()
        .map(str::to_owned)
        .ok_or_else(|| WebDriverError::Shape("value is not a string".into()))
}

/// Typed helper to decode a `value` into a struct.
pub fn decode<T: DeserializeOwned>(value: Value) -> Result<T, WebDriverError> {
    serde_json::from_value(value).map_err(|e| WebDriverError::Shape(e.to_string()))
}

#[cfg(test)]
mod tests;
