//! First launch of the v2 app: a three-slide walkthrough (skip or done)
//! lands on the order book. Node selection lives in settings, so it is
//! configured afterwards; the marker proves this is a Mortsom build.

use mortsom_app_contract::{AdapterError, DeviceContext};

use super::{expect, present, tap};
use crate::selectors::Ids;

/// Skips the walkthrough and verifies the test-environment marker.
pub async fn complete(device: &DeviceContext<'_>) -> Result<(), AdapterError> {
    if present(device, Ids::WALKTHROUGH_SKIP).await? {
        tap(device, Ids::WALKTHROUGH_SKIP).await?;
    } else if present(device, Ids::WALKTHROUGH_DONE).await? {
        tap(device, Ids::WALKTHROUGH_DONE).await?;
    }
    expect(device, Ids::ORDER_ADD_FAB).await?;
    // The marker may render just after the order book, so wait for it
    // instead of probing once. Its absence means the wrong artifact is
    // installed, not that the app broke the automation contract.
    expect(device, Ids::ENV_MARKER).await.map_err(|_| {
        AdapterError::environment(
            "test-environment marker missing after the walkthrough: this is not a Mortsom build",
        )
    })?;
    Ok(())
}
