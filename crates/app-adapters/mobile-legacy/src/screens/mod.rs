//! Screen objects: small, single-purpose sequences over the UI session.
//! Every wait is bounded by the device's UI timeout; there are no sleeps.

pub mod nav;
pub mod onboarding;
pub mod orders;
pub mod settings;
pub mod wallet;

use mortsom_app_contract::{AdapterError, DeviceContext};

/// Waits for `id` to appear within the device's UI timeout, mapping a
/// missing element to an automation-contract failure.
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

/// Types into `id` after waiting for it. The text may be a secret; it is
/// never included in errors or logs.
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

/// Reads the accessibility label of `id` after waiting for it.
pub async fn label(device: &DeviceContext<'_>, id: &str) -> Result<String, AdapterError> {
    expect(device, id).await?;
    device
        .ui
        .label_of(id)
        .await
        .map_err(|e| AdapterError::infrastructure(format!("read `{id}`: {e}")))
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
