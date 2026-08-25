//! First launch of the v2 app: a three-slide walkthrough (skip or done)
//! lands on the order book. Node selection lives in settings, so it is
//! configured afterwards; the marker proves this is a Mortsom build.

use mortsom_app_contract::{AdapterError, DeviceContext};

use super::{expect, first_present, tap};
use crate::selectors::Ids;

/// Skips the walkthrough and verifies the test-environment marker.
pub async fn complete(device: &DeviceContext<'_>) -> Result<(), AdapterError> {
    // The session opens before the app has drawn anything, so the first
    // look can miss a walkthrough that is on its way. Probing once and
    // moving on read that as "no walkthrough" and then waited out the whole
    // UI timeout on slide one, where the order book never appears. Wait for
    // whichever screen actually arrives -- the walkthrough, or the book on
    // a launch that has none.
    match first_present(
        device,
        &[
            Ids::WALKTHROUGH_SKIP,
            Ids::WALKTHROUGH_DONE,
            Ids::ORDER_ADD_FAB,
        ],
    )
    .await?
    {
        Some(id) if id != Ids::ORDER_ADD_FAB => tap(device, id).await?,
        // The book is already up: the launch had no walkthrough.
        Some(_) => {}
        // Nothing came. That wait already spent the UI timeout, so waiting
        // for the book again would only double how long a stuck app takes
        // to report -- and delay the evidence and the cleanup behind it.
        None => {
            return Err(AdapterError::contract(format!(
                "`{}` did not appear: the app showed neither the walkthrough nor the order book",
                Ids::ORDER_ADD_FAB
            )));
        }
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
