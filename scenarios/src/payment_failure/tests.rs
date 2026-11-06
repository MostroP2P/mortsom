//! The payment-failure family against the fixture daemon: a payout that
//! fails and is replaced, an invoice the daemon refuses, and payouts one
//! payee pins up to the declared cap.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use mortsom_scenario_kit::context::HoldInvoiceState;
use mortsom_scenario_kit::registry;

use crate::flow_fixture::{Fixture, Misbehaviour, ORDER};

fn scenario(name: &str) -> &'static registry::ScenarioDef {
    crate::link();
    registry::find(name).expect("registered")
}

#[tokio::test]
async fn a_withdrawn_payout_is_retried_then_replaced_and_only_the_replacement_settles() {
    let fixture = Fixture::new(false).await;

    (scenario("payout_fails_then_replaced").run)(&fixture.context)
        .await
        .unwrap();

    assert_eq!(
        fixture.actions(),
        [
            "create:alice",
            "take:bob",
            "invoice:bob",
            "hold:alice",
            "pay:alice",
            "fiat:bob",
            "cancel-invoice:bob",
            "release:alice",
            "daemon:ask-invoice",
            "invoice:bob",
        ]
    );
    assert_eq!(
        fixture.relay_states(ORDER),
        ["pending", "in-progress", "success"]
    );
    let invoices = fixture.buyer_invoices();
    assert_eq!(invoices.len(), 2, "one withdrawn, one replacement");
    assert!(
        invoices
            .iter()
            .any(|invoice| invoice.superseded && !invoice.settled),
        "the withdrawn invoice never settled: {invoices:?}"
    );
    assert!(
        invoices
            .iter()
            .any(|invoice| invoice.settled && !invoice.superseded),
        "the replacement settled: {invoices:?}"
    );
    assert!(
        fixture
            .wallet_checks()
            .contains(&"alice:settled".to_owned())
    );
    assert!(fixture.wallet_checks().contains(&"bob:failed".to_owned()));
    assert!(fixture.context.cleanup.report().is_clean());
}

/// A daemon that pays an invoice the buyer withdrew is a product failure:
/// the sats went to an instrument the payee had cancelled.
#[tokio::test]
async fn a_daemon_that_pays_the_withdrawn_invoice_anyway_fails() {
    let fixture = Fixture::with(Misbehaviour {
        pays_withdrawn_invoices: true,
        ..Default::default()
    })
    .await;

    let error = (scenario("payout_fails_then_replaced").run)(&fixture.context)
        .await
        .unwrap_err();

    assert_eq!(error.category, mortsom_common::FailureCategory::Product);
    assert!(
        error.message.contains("while waiting for waiting-invoice"),
        "{error}"
    );
}

#[tokio::test]
async fn an_invoice_below_the_expiry_floor_is_refused_and_a_proper_one_accepted() {
    let fixture = Fixture::new(false).await;

    (scenario("payout_invoice_below_expiry_floor").run)(&fixture.context)
        .await
        .unwrap();

    assert_eq!(
        fixture.actions(),
        [
            "create:alice",
            "take:bob",
            "invoice-refused-by-app:bob",
            "cancel-invoice:bob",
            "invoice:bob",
        ]
    );
    assert_eq!(fixture.relay_states(ORDER), ["pending", "in-progress"]);
    let invoices = fixture.buyer_invoices();
    assert_eq!(invoices.len(), 2);
    assert!(invoices.iter().all(|invoice| !invoice.settled));
    assert_eq!(
        invoices.iter().filter(|invoice| invoice.superseded).count(),
        1
    );
    // No value moved and the order is left to the cleanup sweep.
    assert!(
        !fixture
            .actions()
            .iter()
            .any(|action| action == "hold:alice")
    );
    assert_eq!(fixture.context.cleanup.pending().len(), 1);
}

/// Before the node's capabilities reach the app it cannot judge the
/// expiry, and the short invoice goes to the daemon, which refuses it.
/// That refusal answers the scenario as well as the app's would.
#[tokio::test]
async fn a_short_invoice_the_app_cannot_judge_yet_is_refused_by_the_daemon() {
    let fixture = Fixture::with(Misbehaviour {
        capabilities_arrive_late: true,
        ..Default::default()
    })
    .await;

    (scenario("payout_invoice_below_expiry_floor").run)(&fixture.context)
        .await
        .unwrap();

    assert_eq!(
        fixture.actions(),
        [
            "create:alice",
            "take:bob",
            "invoice-refused:bob",
            "cancel-invoice:bob",
            "invoice:bob",
        ]
    );
    assert_eq!(fixture.relay_states(ORDER), ["pending", "in-progress"]);
}

/// A client with no invoice check (the CLI) sends the short invoice, the
/// daemon refuses it, and the proper one is accepted without a verdict.
#[tokio::test]
async fn a_client_without_an_invoice_check_leaves_the_verdict_to_the_daemon() {
    let fixture = Fixture::with(Misbehaviour {
        lacks_invoice_check: true,
        ..Default::default()
    })
    .await;

    (scenario("payout_invoice_below_expiry_floor").run)(&fixture.context)
        .await
        .unwrap();

    assert_eq!(
        fixture.actions(),
        [
            "create:alice",
            "take:bob",
            "invoice-refused:bob",
            "cancel-invoice:bob",
            "invoice:bob",
        ]
    );
    assert_eq!(fixture.relay_states(ORDER), ["pending", "in-progress"]);
}

/// The app refusing the short invoice for some other rule is not the
/// expiry floor at work.
#[tokio::test]
async fn an_app_that_refuses_a_short_invoice_for_another_rule_fails() {
    let fixture = Fixture::with(Misbehaviour {
        refuses_for_another_rule: true,
        ..Default::default()
    })
    .await;

    let error = (scenario("payout_invoice_below_expiry_floor").run)(&fixture.context)
        .await
        .unwrap_err();

    assert_eq!(error.category, mortsom_common::FailureCategory::Product);
    assert!(error.message.contains("wrong-amount"), "{error}");
}

/// A proper invoice sent without the app ever calling it `valid` proves
/// nothing about the app's check.
#[tokio::test]
async fn an_app_that_never_judges_the_proper_invoice_valid_fails() {
    let fixture = Fixture::with(Misbehaviour {
        never_judges_valid: true,
        ..Default::default()
    })
    .await;

    let error = (scenario("payout_invoice_below_expiry_floor").run)(&fixture.context)
        .await
        .unwrap_err();

    assert_eq!(error.category, mortsom_common::FailureCategory::Product);
    assert!(error.message.contains("valid"), "{error}");
}

/// A daemon that takes an invoice below its own floor is a product failure.
#[tokio::test]
async fn a_daemon_that_accepts_a_short_invoice_fails() {
    let fixture = Fixture::with(Misbehaviour {
        capabilities_arrive_late: true,
        accepts_short_invoices: true,
        ..Default::default()
    })
    .await;

    let error = (scenario("payout_invoice_below_expiry_floor").run)(&fixture.context)
        .await
        .unwrap_err();

    assert_eq!(error.category, mortsom_common::FailureCategory::Product);
    assert!(error.message.contains("should have refused"), "{error}");
}

#[tokio::test]
async fn payouts_to_one_destination_stop_at_the_cap_and_resume_when_a_held_one_settles() {
    let fixture = Fixture::new(false).await;

    (scenario("payout_slots_capped_per_destination").run)(&fixture.context)
        .await
        .unwrap();

    let actions = fixture.actions();
    let count = |verb: &str| actions.iter().filter(|action| *action == verb).count();
    assert_eq!(count("create:alice"), 3, "{actions:?}");
    assert_eq!(count("release:alice"), 3, "{actions:?}");
    assert_eq!(count("daemon:hold-payout"), 1, "{actions:?}");
    assert_eq!(count("settle-hold:bob"), 3, "{actions:?}");
    // The third payout was held back until the first settled.
    let held = actions
        .iter()
        .position(|action| action == "daemon:hold-payout")
        .unwrap();
    let first_settle = actions
        .iter()
        .position(|action| action == "settle-hold:bob")
        .unwrap();
    assert!(held < first_settle, "{actions:?}");
    for order in [ORDER, "the-order-2", "the-order-3"] {
        assert_eq!(
            fixture.relay_states(order),
            ["pending", "in-progress", "success"],
            "{order}"
        );
    }
    let invoices = fixture.buyer_invoices();
    assert_eq!(invoices.len(), 3);
    assert!(
        invoices
            .iter()
            .all(|invoice| invoice.hold == Some(HoldInvoiceState::Settled) && invoice.settled),
        "{invoices:?}"
    );
    assert!(fixture.context.cleanup.report().is_clean());
}

/// A daemon that dispatches past its cap lets one payee pin as many
/// payouts as it likes: the scenario fails on the payout that should
/// have waited.
#[tokio::test]
async fn a_daemon_that_ignores_the_cap_fails() {
    let fixture = Fixture::with(Misbehaviour {
        ignores_payout_cap: true,
        ..Default::default()
    })
    .await;

    let error = (scenario("payout_slots_capped_per_destination").run)(&fixture.context)
        .await
        .unwrap_err();

    assert_eq!(error.category, mortsom_common::FailureCategory::Product);
    assert!(error.message.contains("had to stay open"), "{error}");
    // The failure leaves nothing pinned on Bob's node: every held payout
    // was settled on the way out.
    assert_eq!(
        fixture
            .actions()
            .iter()
            .filter(|action| *action == "settle-hold:bob")
            .count(),
        3,
        "{:?}",
        fixture.actions()
    );
}

/// Without the stack's declared limits there is nothing to test against:
/// the scenarios refuse as environment errors before any order exists.
#[tokio::test]
async fn undeclared_limits_are_an_environment_error_before_anything_is_created() {
    for name in [
        "payout_invoice_below_expiry_floor",
        "payout_slots_capped_per_destination",
    ] {
        let fixture = Fixture::new(false).await.without_payout_limits();
        let error = (scenario(name).run)(&fixture.context).await.unwrap_err();
        assert_eq!(
            error.category,
            mortsom_common::FailureCategory::EnvironmentMisconfiguration,
            "{name}: {error}"
        );
        assert!(
            fixture.actions().is_empty(),
            "{name}: {:?}",
            fixture.actions()
        );
    }
}
