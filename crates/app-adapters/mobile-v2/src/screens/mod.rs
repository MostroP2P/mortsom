//! Screen objects for the v2 app: small, single-purpose sequences over the
//! UI session. Every wait is bounded by the device's UI timeout.

pub mod nav;
pub mod onboarding;
pub mod orders;
pub mod settings;
pub mod wallet;

use mortsom_app_contract::{AdapterError, DeviceContext};

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

/// Whether `id` is present right now.
pub async fn present(device: &DeviceContext<'_>, id: &str) -> Result<bool, AdapterError> {
    device
        .ui
        .exists(id)
        .await
        .map_err(|e| AdapterError::infrastructure(format!("probe `{id}`: {e}")))
}
