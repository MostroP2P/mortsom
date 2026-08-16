//! First launch: walkthrough, then community/node selection through the
//! custom-node dialog with the daemon under test.

use mortsom_app_contract::{AdapterError, DeviceContext, TestEnvironment};

use super::{expect, present, tap, type_into};
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
    dismiss_notice(device).await?;
    select_daemon(device, env).await?;
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

/// Dismisses the legal notice the community screen opens over itself.
///
/// The app raises it one frame after the screen builds, so a single probe
/// can look before it is there and then be blocked by it. This waits for
/// either the notice or the screen behind it — whichever the app shows —
/// instead of racing the frame.
async fn dismiss_notice(device: &DeviceContext<'_>) -> Result<(), AdapterError> {
    let deadline = std::time::Instant::now() + device.ui_timeout;
    loop {
        if present(device, Ids::COMMUNITY_NOTICE_ACCEPT).await? {
            return tap(device, Ids::COMMUNITY_NOTICE_ACCEPT).await;
        }
        // The controls behind the notice are only reachable once it is
        // gone, so seeing one means there is nothing to dismiss.
        if present(device, Ids::COMMUNITY_SKIP).await?
            || present(device, Ids::COMMUNITY_CUSTOM_NODE).await?
        {
            return Ok(());
        }
        if std::time::Instant::now() >= deadline {
            return Ok(());
        }
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    }
}

/// Chooses the daemon under test and leaves the community screen.
///
/// The app discovers Mostro nodes from the relays it already knows, so the
/// daemon may be listed before Mortsom ever mentions it — and adding it as
/// a custom node then fails as a duplicate. Its card is chosen when it is
/// there; only an unknown daemon is added by hand.
async fn select_daemon(
    device: &DeviceContext<'_>,
    env: &TestEnvironment,
) -> Result<(), AdapterError> {
    // Nodes are keyed by lowercase hex.
    let card = Ids::community_card(&env.mostro_pubkey.to_ascii_lowercase());
    if present(device, &card).await? {
        tap(device, &card).await?;
        return tap(device, Ids::COMMUNITY_DONE).await;
    }
    tap(device, Ids::COMMUNITY_CUSTOM_NODE).await?;
    type_into(device, Ids::NODE_CUSTOM_PUBKEY, &env.mostro_pubkey).await?;
    type_into(device, Ids::NODE_CUSTOM_NAME, NODE_NAME).await?;
    tap(device, Ids::NODE_CUSTOM_CONFIRM).await?;
    // Adding a known node is refused and the dialog stays open: the daemon
    // is listed after all, just not where the first look could see it.
    if !present(device, Ids::NODE_CUSTOM_CONFIRM).await? {
        // Adding a custom node selects it; the confirm button then appears.
        return tap(device, Ids::COMMUNITY_DONE).await;
    }
    // The app knows this node already, it just does not offer it here: the
    // community screen lists communities, and a plain node is not one. Leave
    // the step; settings is where a known node is chosen.
    tap(device, Ids::NODE_CUSTOM_CANCEL).await?;
    tap(device, Ids::COMMUNITY_SKIP).await
}
