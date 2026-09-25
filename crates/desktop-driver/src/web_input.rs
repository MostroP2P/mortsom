//! Public DOM scrolling, W3C editing and value-property verification.
use crate::{
    text_entry::shows,
    web::{error, selector},
};
use mortsom_ui_driver::{
    error::UiError,
    webdriver::{Element, Locator, Session, WebDriverError},
};
use serde_json::json;
use std::time::Duration;

async fn input(session: &Session, id: &str) -> Result<Element, UiError> {
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

pub(crate) async fn read(session: &Session, id: &str) -> Result<String, UiError> {
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
///
/// `timeout` bounds the wait for a field at rest and the start of each new
/// edit, never an edit already sent: a slow browser can take seconds to
/// answer the focusing click and the keys, and an edit cut short before its
/// read-back would report as a mismatch a value the field already shows.
pub(crate) async fn enter(
    session: &Session,
    id: &str,
    text: &str,
    timeout: Duration,
) -> Result<(), UiError> {
    let deadline = tokio::time::Instant::now() + timeout;
    let result = entered(session, id, text, deadline).await;
    let redactor = mortsom_common::Redactor::new().with_secret(&mortsom_common::Secret::new(text));
    result.map_err(|cause| error(redactor.redact(&cause.to_string())))
}

async fn entered(
    session: &Session,
    id: &str,
    text: &str,
    deadline: tokio::time::Instant,
) -> Result<(), UiError> {
    let mut tick = tokio::time::interval(Duration::from_millis(100));
    loop {
        tick.tick().await;
        if tokio::time::Instant::now() >= deadline {
            return Err(error("text-entry deadline: public value mismatch"));
        }
        match replace(session, id, text, deadline).await {
            Ok(true) => return Ok(()),
            Ok(false) => {}
            Err(cause) if transient(&cause) => {}
            Err(cause) => return Err(cause),
        }
    }
}

/// One attempt: `true` once the field shows `text`. Only the look before
/// the edit is cut at `deadline`; an edit that has begun is read back.
async fn replace(
    session: &Session,
    id: &str,
    text: &str,
    deadline: tokio::time::Instant,
) -> Result<bool, UiError> {
    let ready = tokio::time::timeout_at(deadline, ready_to_edit(session, id, text)).await;
    match ready {
        Ok(Ok(Edit::Needed)) => {}
        Ok(Ok(Edit::Shown)) => return Ok(true),
        Ok(Ok(Edit::Moving)) | Err(_) => return Ok(false),
        Ok(Err(cause)) => return Err(cause),
    }
    session.click(&input(session, id).await?).await?;
    session.clear(&input(session, id).await?).await?;
    session.send_keys(&input(session, id).await?, text).await?;
    Ok(shows(&read(session, id).await?, text))
}

/// What the field needs before an edit may be sent.
enum Edit {
    /// It already shows the text.
    Shown,
    /// It is still moving; clicking it now could land on a neighbour.
    Moving,
    /// It rests and shows something else.
    Needed,
}

async fn ready_to_edit(session: &Session, id: &str, text: &str) -> Result<Edit, UiError> {
    if shows(&read(session, id).await?, text) {
        return Ok(Edit::Shown);
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
    // The focusing click must land on the field, not on whatever is passing
    // through its place: a dialog that has just closed is still animating
    // the form back into position, and a click delivered then opens the
    // neighbouring control instead. Wait for the field to rest first.
    let before = rect(session, &node).await?;
    tokio::time::sleep(Duration::from_millis(100)).await;
    if rect(session, &node).await? != before {
        return Ok(Edit::Moving);
    }
    Ok(Edit::Needed)
}

/// Presses `id` once, after it has come to rest.
///
/// Flutter web rebuilds its semantics nodes after a scroll or a focus
/// change, and WebDriver clicks the centre of the element it was handed at
/// the moment it was handed it. A click that lands while the node is still
/// moving, or on a node the framework has just replaced, is delivered to
/// nothing -- which a live run showed as a filled order form whose submit
/// button had been "clicked" and never left. So: scroll the node into view,
/// read its rectangle twice and click only when the two agree and the node
/// has a size. Errors that mean the click was never delivered are retried
/// until `timeout`; a delivered click is never repeated, because the
/// business action behind it may already be on its way.
///
/// `timeout` bounds the wait for a resting node, never a click already
/// sent: a slow browser can take seconds to answer one, and cutting it
/// short would report as unsent a click the app may have acted on.
pub(crate) async fn press(session: &Session, id: &str, timeout: Duration) -> Result<(), UiError> {
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        let node = resting(session, id, deadline).await?;
        match session.click(&node).await.map_err(UiError::from) {
            Ok(()) => return Ok(()),
            Err(cause) if transient(&cause) => {}
            Err(cause) => return Err(cause),
        }
    }
}

/// The node of `id` once it rests with a size, looked for until `deadline`.
async fn resting(
    session: &Session,
    id: &str,
    deadline: tokio::time::Instant,
) -> Result<Element, UiError> {
    tokio::time::timeout_at(deadline, async {
        let mut tick = tokio::time::interval(Duration::from_millis(100));
        loop {
            tick.tick().await;
            match resting_once(session, id).await {
                Ok(Some(node)) => return Ok(node),
                Ok(None) => {}
                Err(cause) if transient(&cause) => {}
                Err(cause) => return Err(cause),
            }
        }
    })
    .await
    .unwrap_or_else(|_| Err(error(format!("press deadline: `{id}` never came to rest"))))
}

/// One look: `None` when the node was still moving or had no size.
async fn resting_once(session: &Session, id: &str) -> Result<Option<Element>, UiError> {
    let node = session.find_element(Locator::XPath, &selector(id)?).await?;
    session
        .post(
            "execute/sync",
            &json!({
                "script":"arguments[0].scrollIntoView({block: 'center', inline: 'nearest'});",
                "args":[{"element-6066-11e4-a52e-4f735466cecf":node.id}]
            }),
        )
        .await?;
    let before = rect(session, &node).await?;
    tokio::time::sleep(Duration::from_millis(100)).await;
    let after = rect(session, &node).await?;
    if before != after || after.2 <= 0.0 || after.3 <= 0.0 {
        return Ok(None);
    }
    Ok(Some(node))
}

/// `(x, y, width, height)` of the node as the browser lays it out now.
async fn rect(session: &Session, node: &Element) -> Result<(f64, f64, f64, f64), UiError> {
    let value = session.get(&format!("element/{}/rect", node.id)).await?;
    let number = |key: &str| value.get(key).and_then(serde_json::Value::as_f64);
    match (number("x"), number("y"), number("width"), number("height")) {
        (Some(x), Some(y), Some(w), Some(h)) => Ok((x, y, w, h)),
        _ => Err(error("element rect is not four numbers")),
    }
}

fn transient(cause: &UiError) -> bool {
    matches!(cause, UiError::WebDriver(WebDriverError::Server {error, ..})
        if matches!(error.as_str(), "no such element" | "stale element reference" |
            "element not interactable" | "element click intercepted"))
}

#[cfg(test)]
#[path = "web_input_tests.rs"]
mod tests;
