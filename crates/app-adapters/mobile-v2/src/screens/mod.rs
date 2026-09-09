//! Screen objects for the v2 app: small, single-purpose sequences over the
//! UI session. Every wait is bounded by the device's UI timeout.

pub mod nav;
pub mod onboarding;
pub mod orders;
pub mod settings;
pub mod wallet;

use mortsom_app_contract::{AdapterError, DeviceContext};
use tracing::warn;

/// Gap between looks while waiting for one of several screens.
pub(super) const SCREEN_POLL: std::time::Duration = std::time::Duration::from_millis(250);

/// How long a control that only has to draw has to answer before the tap
/// is repeated.
const TAP_PATIENCE: std::time::Duration = std::time::Duration::from_secs(3);

/// How many times a tap is repeated before it is a contract failure.
const TAP_ATTEMPTS: usize = 3;

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

/// Taps `control` until `expected` shows up, or reports that it never did.
///
/// A tap that lands while the app is rebuilding a screen is swallowed: the
/// control is on screen, the driver hits it, and nothing happens -- a
/// keyboard closing under the button as it is pressed, or a screen still
/// settling after a relaunch. Waiting longer cannot help, because nobody
/// is going to act on the tap that was dropped. Asking again is what does.
pub async fn tap_until(
    device: &DeviceContext<'_>,
    control: &str,
    expected: &str,
) -> Result<(), AdapterError> {
    for _ in 0..TAP_ATTEMPTS {
        tap(device, control).await?;
        if appears_within(device, expected, TAP_PATIENCE).await? {
            return Ok(());
        }
    }
    Err(AdapterError::contract(format!(
        "`{expected}` did not appear after tapping `{control}` {TAP_ATTEMPTS} times"
    )))
}

/// [`tap_until`] for a control that must never act twice: it asks again
/// only while `control` is still there to be pressed.
///
/// Submit is that kind of button. The first press publishes a real order
/// on mainnet, and a second one publishes another that the run never sees
/// in the trades list and never cancels in cleanup. Publishing also waits
/// on the network, so a confirmation that has not drawn is not evidence of
/// anything -- while a control still sitting on screen, still enabled, is
/// the app saying it acted on nothing. That is the only thing worth
/// re-tapping on; once the app has taken the tap, waiting is all that is
/// left, and `expected` is waited for over the whole UI budget.
pub async fn tap_unless_taken(
    device: &DeviceContext<'_>,
    control: &str,
    expected: &str,
    patience: std::time::Duration,
) -> Result<(), AdapterError> {
    for _ in 0..TAP_ATTEMPTS {
        tap(device, control).await?;
        if appears_within(device, expected, patience).await? {
            return Ok(());
        }
        if !dropped(device, control).await? {
            return expect(device, expected).await;
        }
    }
    Err(AdapterError::contract(format!(
        "`{expected}` did not appear after tapping `{control}` {TAP_ATTEMPTS} times"
    )))
}

/// Whether `control` looks like it was never acted on: still on screen and
/// still accepting a press. An app that took the tap has either left the
/// screen behind or disabled the control while it works.
async fn dropped(device: &DeviceContext<'_>, control: &str) -> Result<bool, AdapterError> {
    if !present(device, control).await? {
        return Ok(false);
    }
    device
        .ui
        .is_enabled(control)
        .await
        .map_err(|e| AdapterError::infrastructure(format!("probe `{control}` enabled: {e}")))
}

/// Whether `id` shows up within `patience`.
async fn appears_within(
    device: &DeviceContext<'_>,
    id: &str,
    patience: std::time::Duration,
) -> Result<bool, AdapterError> {
    let deadline = tokio::time::Instant::now() + patience;
    loop {
        if present(device, id).await? {
            return Ok(true);
        }
        if tokio::time::Instant::now() >= deadline {
            return Ok(false);
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
///
/// Best effort: a driver that will not put a keyboard away -- or that
/// objects because there was none up to begin with -- must not fail a run
/// on its own. What the caller actually needs is the control underneath,
/// and the wait for it says so precisely if the keyboard really was in the
/// way.
pub async fn dismiss_keyboard(device: &DeviceContext<'_>) {
    if let Err(e) = device.ui.hide_keyboard().await {
        warn!(error = %e, "could not put the keyboard away");
    }
}

/// Picks `option` from a picker and waits for the picker, identified by
/// `picker`, to close. Choosing is not a business action -- picking the same
/// currency twice selects it once -- so a pick the picker did not act on is
/// pressed again, a bounded number of times. Flutter web rebuilds a lazy
/// list's rows while it filters, and a press delivered to a row it has just
/// replaced reaches nothing.
pub async fn choose(
    device: &DeviceContext<'_>,
    option: &str,
    picker: &str,
) -> Result<(), AdapterError> {
    const PICKS: usize = 3;
    for _ in 0..PICKS {
        tap(device, option).await?;
        if gone_within(device, picker, PICKER_CLOSE).await? {
            return Ok(());
        }
    }
    Err(AdapterError::contract(format!(
        "`{picker}` stayed open after choosing `{option}` {PICKS} times"
    )))
}

/// How long a picker gets to close after a choice before the choice is
/// believed lost.
const PICKER_CLOSE: std::time::Duration = std::time::Duration::from_secs(3);

/// Whether `id` disappears within `patience`.
async fn gone_within(
    device: &DeviceContext<'_>,
    id: &str,
    patience: std::time::Duration,
) -> Result<bool, AdapterError> {
    let deadline = tokio::time::Instant::now() + patience;
    loop {
        if !present(device, id).await? {
            return Ok(true);
        }
        if tokio::time::Instant::now() >= deadline {
            return Ok(false);
        }
        tokio::time::sleep(SCREEN_POLL).await;
    }
}

/// Whether `id` shows up anywhere in a list within `patience`. A list is
/// drawn after its route: what is absent on the first probe may be there on
/// the next, so a single `listed` call reads a loading screen as an answer.
pub async fn listed_within(
    device: &DeviceContext<'_>,
    id: &str,
    patience: std::time::Duration,
) -> Result<bool, AdapterError> {
    let deadline = tokio::time::Instant::now() + patience;
    loop {
        if listed(device, id).await? {
            return Ok(true);
        }
        if tokio::time::Instant::now() >= deadline {
            return Ok(false);
        }
        tokio::time::sleep(SCREEN_POLL).await;
    }
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
