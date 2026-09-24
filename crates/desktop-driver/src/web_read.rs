//! Reads of Flutter Web's public semantics DOM.
//!
//! A read is two requests: find the node, then ask it for a value. Flutter
//! Web rebuilds the semantics of a screen that redraws -- the trade detail
//! redraws every second for its countdown -- and a node replaced between
//! the two answers `stale element reference`. Its identifier then names
//! the replacement, so the read is simply made again from the find. A read
//! has no effect on the app, so repeating it is always safe; only a stale
//! node is looked for again, and only until the limit.
use crate::web::selector;
use mortsom_ui_driver::{
    error::UiError,
    webdriver::{Locator, Session},
};
use std::future::Future;
use std::time::Duration;

/// The accessibility label of `id`, or its visible text when it has none.
pub(crate) async fn label(session: &Session, id: &str, limit: Duration) -> Result<String, UiError> {
    again_while_stale(limit, || async {
        let node = session.find_element(Locator::XPath, &selector(id)?).await?;
        let label = session
            .get(&format!("element/{}/attribute/aria-label", node.id))
            .await?;
        match label.as_str() {
            Some(value) => Ok(value.to_owned()),
            None => Ok(session.text(&node).await?.trim().to_owned()),
        }
    })
    .await
}

/// Whether `id` is enabled, by its ARIA state and the browser's own.
pub(crate) async fn enabled(session: &Session, id: &str, limit: Duration) -> Result<bool, UiError> {
    again_while_stale(limit, || async {
        let node = session.find_element(Locator::XPath, &selector(id)?).await?;
        let disabled = session
            .get(&format!("element/{}/attribute/aria-disabled", node.id))
            .await?;
        Ok(disabled.as_str() != Some("true") && session.is_enabled(&node).await?)
    })
    .await
}

/// The value of the text field `id`, or the rendered text of a node that
/// has no field.
pub(crate) async fn text(session: &Session, id: &str, limit: Duration) -> Result<String, UiError> {
    again_while_stale(limit, || async {
        match crate::web_input::read(session, id).await {
            Err(UiError::WebDriver(cause)) if cause.is_no_such_element() => {
                let node = session.find_element(Locator::XPath, &selector(id)?).await?;
                Ok(session.text(&node).await?)
            }
            result => result,
        }
    })
    .await
}

/// Runs `read`, and runs it again while it fails on a replaced node, until
/// `limit`. Every other outcome is the answer.
async fn again_while_stale<T, F, Fut>(limit: Duration, mut read: F) -> Result<T, UiError>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<T, UiError>>,
{
    let deadline = tokio::time::Instant::now() + limit;
    let mut tick = tokio::time::interval(Duration::from_millis(100));
    loop {
        tick.tick().await;
        match read().await {
            Err(UiError::WebDriver(cause))
                if cause.is_stale_element() && tokio::time::Instant::now() < deadline => {}
            result => return result,
        }
    }
}

#[cfg(test)]
#[path = "web_read_tests.rs"]
mod tests;
