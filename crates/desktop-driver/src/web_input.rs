//! Public DOM scrolling, W3C editing and value-property verification.
use crate::web::{error, selector};
use mortsom_android_driver::{
    appium::AppiumError,
    webdriver::{Element, Locator, Session, WebDriverError},
};
use serde_json::json;
use std::time::Duration;

async fn input(session: &Session, id: &str) -> Result<Element, AppiumError> {
    Ok(session
        .find_element(
            Locator::XPath,
            &format!(
                "{}/descendant-or-self::*[self::input or self::textarea]",
                selector(id)?
            ),
        )
        .await?)
}

pub(crate) async fn read(session: &Session, id: &str) -> Result<String, AppiumError> {
    let node = input(session, id).await?;
    let value = session
        .get(&format!("element/{}/property/value", node.id))
        .await?;
    value
        .as_str()
        .map(str::to_owned)
        .ok_or_else(|| error("editable node has no string value"))
}

/// Text replacement is idempotent; every transition uses a new lookup.
pub(crate) async fn enter(
    session: &Session,
    id: &str,
    text: &str,
    timeout: Duration,
) -> Result<(), AppiumError> {
    let result = tokio::time::timeout(timeout, async {
        let mut tick = tokio::time::interval(Duration::from_millis(100));
        loop {
            tick.tick().await;
            match replace(session, id, text).await {
                Ok(true) => return Ok(()),
                Ok(false) => {}
                Err(cause) if transient(&cause) => {}
                Err(cause) => return Err(cause),
            }
        }
    })
    .await
    .unwrap_or_else(|_| Err(error("text-entry deadline: public value mismatch")));
    let redactor = mortsom_common::Redactor::new().with_secret(&mortsom_common::Secret::new(text));
    result.map_err(|cause| error(redactor.redact(&cause.to_string())))
}

async fn replace(session: &Session, id: &str, text: &str) -> Result<bool, AppiumError> {
    if read(session, id).await? == text {
        return Ok(true);
    }
    let node = session.find_element(Locator::XPath, &selector(id)?).await?;
    // Only the public DOM scroll operation; no app state or event injection.
    session
        .post(
            "execute/sync",
            &json!({
                "script":"arguments[0].scrollIntoView({block: 'center', inline: 'nearest'});",
                "args":[{"element-6066-11e4-a52e-4f735466cecf":node.id}]
            }),
        )
        .await?;
    session.click(&input(session, id).await?).await?;
    session.clear(&input(session, id).await?).await?;
    session.send_keys(&input(session, id).await?, text).await?;
    Ok(read(session, id).await? == text)
}

fn transient(cause: &AppiumError) -> bool {
    matches!(cause, AppiumError::WebDriver(WebDriverError::Server {error, ..})
        if matches!(error.as_str(), "no such element" | "stale element reference" |
            "element not interactable" | "element click intercepted"))
}

#[cfg(test)]
#[path = "web_input_tests.rs"]
mod tests;
