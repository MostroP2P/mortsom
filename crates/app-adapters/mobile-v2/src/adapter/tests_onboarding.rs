//! The first run ends on a node choice: the adapter picks the daemon under
//! test and confirms it. Skip would keep the app's built-in default node,
//! which in a Mortsom build is not the stack's daemon.
use super::*;

/// The node choice's skip link: never pressed by the adapter.
const COMMUNITY_SKIP: &str = "onboarding.community.skip";

/// A fresh app whose walkthrough leads to the node choice; "Use this node"
/// stays disabled until a card is picked, and confirming lands on the book.
fn app_with_node_choice() -> FakeUi {
    let ui = fresh_app();
    let card = Ids::community_card(PUBKEY);
    let choice = [Ids::COMMUNITY_DONE, COMMUNITY_SKIP, card.as_str()];
    ui.on_tap(Ids::WALKTHROUGH_SKIP, &choice);
    ui.disables(&[Ids::COMMUNITY_DONE]);
    ui.on_tap_enable(&card, &[Ids::COMMUNITY_DONE]);
    ui.on_tap(
        Ids::COMMUNITY_DONE,
        &[Ids::ORDER_ADD_FAB, Ids::ENV_MARKER, Ids::APPBAR_DRAWER],
    );
    ui.on_tap_hide(Ids::COMMUNITY_DONE, &choice);
    ui
}

#[tokio::test]
async fn onboarding_picks_the_daemon_under_test_on_the_node_choice() {
    let ui = app_with_node_choice();

    let identity = adapter().onboard(&device(&ui), &env()).await.unwrap();

    assert_eq!(identity.public_key, "npub1alice");
    let calls = ui.calls();
    let card = format!("tap {}", Ids::community_card(PUBKEY));
    let done = format!("tap {}", Ids::COMMUNITY_DONE);
    let picked = calls.iter().position(|c| *c == card).expect("card picked");
    let confirmed = calls.iter().position(|c| *c == done).expect("confirmed");
    assert!(picked < confirmed, "{calls:?}");
    assert!(
        !calls.contains(&format!("tap {COMMUNITY_SKIP}")),
        "{calls:?}"
    );
}

#[tokio::test]
async fn a_pick_the_app_dropped_is_made_again() {
    let ui = app_with_node_choice();
    ui.swallows_taps(&Ids::community_card(PUBKEY), 1);

    adapter().onboard(&device(&ui), &env()).await.unwrap();

    let card = format!("tap {}", Ids::community_card(PUBKEY));
    assert_eq!(ui.calls().iter().filter(|c| **c == card).count(), 2);
}

#[tokio::test]
async fn a_node_choice_that_never_takes_the_pick_is_a_contract_failure() {
    let ui = app_with_node_choice();
    ui.on_tap_enable(&Ids::community_card(PUBKEY), &[]);

    let error = adapter().onboard(&device(&ui), &env()).await.unwrap_err();

    assert_eq!(
        error.category,
        mortsom_common::FailureCategory::AutomationContract
    );
    assert!(error.message.contains(Ids::COMMUNITY_DONE), "{error:?}");
    assert!(!ui.calls().contains(&format!("tap {}", Ids::COMMUNITY_DONE)));
}
