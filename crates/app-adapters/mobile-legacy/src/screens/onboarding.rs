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
    // The community screen opens a legal notice over itself on first launch
    // and nothing behind it can be tapped. A build that already accepted it
    // (or one that does not show it) simply has nothing to dismiss.
    if present(device, Ids::COMMUNITY_NOTICE_ACCEPT).await? {
        tap(device, Ids::COMMUNITY_NOTICE_ACCEPT).await?;
    }
    select_daemon(device, env).await?;
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

/// Selects the daemon under test on the community screen.
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
        return tap(device, &card).await;
    }
    tap(device, Ids::COMMUNITY_CUSTOM_NODE).await?;
    type_into(device, Ids::NODE_CUSTOM_PUBKEY, &env.mostro_pubkey).await?;
    type_into(device, Ids::NODE_CUSTOM_NAME, NODE_NAME).await?;
    tap(device, Ids::NODE_CUSTOM_CONFIRM).await?;
    // Adding a known node is refused and the dialog stays open: the daemon
    // is listed after all, just not where the first look could see it.
    if !present(device, Ids::NODE_CUSTOM_CONFIRM).await? {
        return Ok(());
    }
    tap(device, Ids::NODE_CUSTOM_CANCEL).await?;
    if present(device, &card).await? {
        return tap(device, &card).await;
    }
    Err(AdapterError::contract(format!(
        "the daemon under test is neither addable as a custom node nor listed as `{card}`"
    )))
}
