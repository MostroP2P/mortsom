//! First launch of the v2 app: a three-slide walkthrough (skip or done),
//! then -- since the first run asks for one -- the node to trade on, and
//! the order book. The marker proves this is a Mortsom build.

use mortsom_app_contract::{AdapterError, DeviceContext, TestEnvironment};

use super::{
    SCREEN_POLL, TAP_ATTEMPTS, TAP_PATIENCE, expect, first_present, tap, tap_unless_taken,
};
use crate::selectors::Ids;

/// Skips the walkthrough, picks the daemon under test, and verifies the
/// test-environment marker.
pub async fn complete(
    device: &DeviceContext<'_>,
    env: &TestEnvironment,
) -> Result<(), AdapterError> {
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
    // A build from before the node choice goes straight to the book.
    if first_present(device, &[Ids::COMMUNITY_DONE, Ids::ORDER_ADD_FAB]).await?
        == Some(Ids::COMMUNITY_DONE)
    {
        choose_node(device, env).await?;
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

/// Picks the daemon under test on the node choice and confirms it, as a
/// user of this stack would. Skip is never pressed: it keeps the app's
/// built-in default node, which in a Mortsom build is not the stack's.
///
/// "Use this node" stays disabled until a card is picked, and a pick that
/// lands while the list rebuilds does nothing, so the pick is made again
/// until the app takes it. Picking the same card twice picks it once.
async fn choose_node(
    device: &DeviceContext<'_>,
    env: &TestEnvironment,
) -> Result<(), AdapterError> {
    let card = Ids::community_card(env.mostro_pubkey.trim());
    for _ in 0..TAP_ATTEMPTS {
        tap(device, &card).await?;
        if enabled_within(device, Ids::COMMUNITY_DONE, TAP_PATIENCE).await? {
            return tap_unless_taken(
                device,
                Ids::COMMUNITY_DONE,
                Ids::ORDER_ADD_FAB,
                TAP_PATIENCE,
            )
            .await;
        }
    }
    Err(AdapterError::contract(format!(
        "`{}` stayed disabled after picking `{card}` {TAP_ATTEMPTS} times",
        Ids::COMMUNITY_DONE
    )))
}

/// Whether `id` accepts a press within `patience`.
async fn enabled_within(
    device: &DeviceContext<'_>,
    id: &str,
    patience: std::time::Duration,
) -> Result<bool, AdapterError> {
    let deadline = tokio::time::Instant::now() + patience;
    loop {
        let enabled = device
            .ui
            .is_enabled(id)
            .await
            .map_err(|e| AdapterError::infrastructure(format!("probe `{id}` enabled: {e}")))?;
        if enabled {
            return Ok(true);
        }
        if tokio::time::Instant::now() >= deadline {
            return Ok(false);
        }
        tokio::time::sleep(SCREEN_POLL).await;
    }
}
