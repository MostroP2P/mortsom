//! Screen objects for the v2 app: small, single-purpose sequences over the
//! UI session. Every wait is bounded by the device's UI timeout.

pub mod nav;
pub mod onboarding;
pub mod orders;
pub mod settings;
pub mod wallet;

use mortsom_app_contract::{AdapterError, DeviceContext};

/// Gap between looks while waiting for one of several screens.
const SCREEN_POLL: std::time::Duration = std::time::Duration::from_millis(250);

/// Waits for `id` within the UI timeout; a missing element is an
/// automation-contract failure.
pub async fn expect(device: &DeviceContext<'_>, id: &str) -> Result<(), AdapterError> {
    device
        .ui
        .wait_for(id, device.ui_timeout)
        .await
        .map_err(|e| AdapterError::contract(format!("`{id}` did not appear: {e}")))
}

/// Taps `id` after waiting for it.
pub async fn tap(device: &DeviceContext<'_>, id: &str) -> Result<(), AdapterError> {
    expect(device, id).await?;
    device
        .ui
        .tap(id)
        .await
        .map_err(|e| AdapterError::infrastructure(format!("tap `{id}`: {e}")))
}

/// Types into `id`; the text may be a secret and never reaches errors.
pub async fn type_into(
    device: &DeviceContext<'_>,
    id: &str,
    text: &str,
) -> Result<(), AdapterError> {
    expect(device, id).await?;
    device
        .ui
        .type_text(id, text)
        .await
        .map_err(|e| AdapterError::infrastructure(format!("type into `{id}`: {e}")))
}

/// Reads the accessibility label of `id`.
pub async fn label(device: &DeviceContext<'_>, id: &str) -> Result<String, AdapterError> {
    expect(device, id).await?;
    device
        .ui
        .label_of(id)
        .await
        .map_err(|e| AdapterError::infrastructure(format!("read `{id}`: {e}")))
}

/// Waits until one of `ids` is on screen and answers which, or `None`
/// when the UI timeout passes with none of them there.
///
/// Ask this when the app may legitimately land on more than one screen and
/// the caller acts differently on each. A single probe per candidate
/// cannot: it reads an app that is still drawing as an app that showed
/// none of them, and every branch after that is a guess.
pub async fn first_present<'a>(
    device: &DeviceContext<'_>,
    ids: &[&'a str],
) -> Result<Option<&'a str>, AdapterError> {
    let deadline = tokio::time::Instant::now() + device.ui_timeout;
    loop {
        for id in ids {
            if present(device, id).await? {
                return Ok(Some(id));
            }
        }
        if tokio::time::Instant::now() >= deadline {
            return Ok(None);
        }
        tokio::time::sleep(SCREEN_POLL).await;
    }
}

/// Puts the soft keyboard away, so the controls it covers are reachable.
///
/// Typing into a field raises it over the bottom of the form. That is not
/// an element hidden below the fold: the keyboard shrinks the viewport and
/// the app's semantics stop there, so the control underneath is absent and
/// no scrolling look finds it.
pub async fn dismiss_keyboard(device: &DeviceContext<'_>) -> Result<(), AdapterError> {
    device
        .ui
        .hide_keyboard()
        .await
        .map_err(|e| AdapterError::infrastructure(format!("hide the keyboard: {e}")))
}

/// Whether `id` is present right now.
pub async fn present(device: &DeviceContext<'_>, id: &str) -> Result<bool, AdapterError> {
    device
        .ui
        .exists(id)
        .await
        .map_err(|e| AdapterError::infrastructure(format!("probe `{id}`: {e}")))
}

/// Whether `id` is anywhere on the current screen, including below the
/// fold. Ask this about a list -- a node in the selector, a relay row in
/// settings -- where what is visible is only part of the answer and the
/// caller acts on it either way. It may scroll to answer, so it is not the
/// probe to use for "which screen am I on".
pub async fn listed(device: &DeviceContext<'_>, id: &str) -> Result<bool, AdapterError> {
    device
        .ui
        .exists_anywhere(id)
        .await
        .map_err(|e| AdapterError::infrastructure(format!("look for `{id}` in the list: {e}")))
}
