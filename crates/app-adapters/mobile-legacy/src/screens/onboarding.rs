//! First launch: walkthrough, then community/node selection through the
//! custom-node dialog with the daemon under test.

use mortsom_app_contract::{AdapterError, DeviceContext, TestEnvironment};

use super::{expect, tap, type_into};
use crate::selectors::Ids;

/// Display name given to the daemon under test in the app.
pub const NODE_NAME: &str = "Mortsom";

/// Completes the walkthrough and selects the daemon under test as a custom
/// node, landing on the order book.
pub async fn complete(
    device: &DeviceContext<'_>,
    env: &TestEnvironment,
) -> Result<(), AdapterError> {
    tap(device, Ids::ONBOARDING_SKIP).await?;
    tap(device, Ids::COMMUNITY_CUSTOM_NODE).await?;
    type_into(device, Ids::NODE_CUSTOM_PUBKEY, &env.mostro_pubkey).await?;
    type_into(device, Ids::NODE_CUSTOM_NAME, NODE_NAME).await?;
    tap(device, Ids::NODE_CUSTOM_CONFIRM).await?;
    // Adding a custom node selects it; the confirm button then appears.
    tap(device, Ids::COMMUNITY_DONE).await?;
    expect(device, Ids::ORDER_ADD_FAB).await?;
    // The marker may render just after the order book, so wait for it
    // instead of probing once. Its absence means the wrong artifact is
    // installed, not that the app broke the automation contract.
    expect(device, Ids::ENV_MARKER).await.map_err(|_| {
        AdapterError::environment(
            "test-environment marker missing after onboarding: this is not a Mortsom build",
        )
    })?;
    Ok(())
}
