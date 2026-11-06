//! `message list` with `data.truncated`: a list that may be incomplete
//! proves no absence. A wait keeps polling, and a verdict or a label that
//! rests on an absence is an inconclusive relay failure.
use super::*;

/// `order info` reporting `o1` in `status`.
fn reported(status: &str) -> serde_json::Value {
    json!({"orders": [{"id": "o1", "status": status}]})
}

/// The node's request for the seller's hold invoice, as `message list`
/// prints it.
fn hold_request() -> serde_json::Value {
    json!({"action": "pay-invoice", "next_step": "pay_hold_invoice", "created_at_unix": "100",
        "payload": {"kind": "payment_request", "invoice": "lnbcrt1hold", "amount": null, "order": null}})
}

#[tokio::test]
async fn a_request_that_never_comes_is_a_product_failure() {
    let (cli, session) = session();
    cli.always("message.list", json!({"messages": [], "truncated": false}));

    let error = adapter()
        .pay_hold_invoice(
            &device(&session, Actor::Alice),
            &order(OrderKind::Sell),
            PaymentPath::ExternalWallet,
        )
        .await
        .unwrap_err();

    assert_eq!(error.category, FailureCategory::Product);
}

#[tokio::test]
async fn a_wait_keeps_polling_past_a_truncated_list() {
    let (cli, session) = session();
    cli.ok("message.list", json!({"messages": [], "truncated": true}));
    cli.always(
        "message.list",
        json!({"messages": [hold_request()], "truncated": true}),
    );

    let handoff = adapter()
        .pay_hold_invoice(
            &device(&session, Actor::Alice),
            &order(OrderKind::Sell),
            PaymentPath::ExternalWallet,
        )
        .await
        .unwrap();

    assert_eq!(handoff.invoice.unwrap().expose(), "lnbcrt1hold");
    assert_eq!(cli.commands(), ["message.list", "message.list"]);
}

#[tokio::test]
async fn a_request_hidden_by_a_truncated_list_is_a_relay_failure() {
    let (cli, session) = session();
    cli.always("message.list", json!({"messages": [], "truncated": true}));

    let error = adapter()
        .pay_hold_invoice(
            &device(&session, Actor::Alice),
            &order(OrderKind::Sell),
            PaymentPath::ExternalWallet,
        )
        .await
        .unwrap_err();

    assert_eq!(error.category, FailureCategory::Relay);
    assert!(error.message.contains("truncated"), "{}", error.message);
}

#[tokio::test]
async fn a_wait_whose_deadline_passes_before_any_read_ends_is_a_relay_failure() {
    let slow = Arc::new(mortsom_cli_driver::testing::SlowCli {
        delay: Duration::from_secs(3600),
    });
    let session = session_over(slow);

    let error = adapter()
        .pay_hold_invoice(
            &device(&session, Actor::Alice),
            &order(OrderKind::Sell),
            PaymentPath::ExternalWallet,
        )
        .await
        .unwrap_err();

    assert_eq!(error.category, FailureCategory::Relay);
    assert!(error.message.contains("o1"), "{}", error.message);
    assert!(error.message.contains("message list"), "{}", error.message);
}

#[tokio::test]
async fn a_truncated_list_proves_a_standing_cancel_but_not_a_strangers() {
    let (cli, session) = session();
    for messages in [
        json!({"messages": [{"action": "cooperative-cancel-initiated-by-you"}], "truncated": true}),
        json!({"messages": [{"action": "buyer-took-order"}], "truncated": true}),
    ] {
        cli.ok(
            "order.info",
            json!({"orders": [{"id": "o1", "status": "active"}]}),
        );
        cli.refuse("order.cancel", "invalid_pubkey");
        cli.ok("message.list", messages);
    }
    let device = device(&session, Actor::Alice);

    let standing = adapter()
        .cancel_if_open(&device, &order(OrderKind::Sell))
        .await
        .unwrap();
    let stranger = adapter()
        .cancel_if_open(&device, &order(OrderKind::Sell))
        .await;

    assert_eq!(standing, CancelOutcome::Cancelled);
    assert_eq!(stranger.unwrap_err().category, FailureCategory::Relay);
}

#[tokio::test]
async fn labels_resting_on_an_absence_are_inconclusive_on_a_truncated_list() {
    let (cli, session) = session();
    for (status, messages) in [
        ("success", json!([{"action": "rate"}])),
        ("success", json!([])),
        (
            "success",
            json!([{"action": "rate"}, {"action": "rate-received"}]),
        ),
        (
            "settled-hold-invoice",
            json!([{"action": "add-invoice", "next_step": "add_invoice"}]),
        ),
        ("settled-hold-invoice", json!([])),
    ] {
        cli.ok("order.info", reported(status));
        cli.ok(
            "message.list",
            json!({"messages": messages, "truncated": true}),
        );
    }
    let device = device(&session, Actor::Bob);
    let mut seen = Vec::new();

    for _ in 0..5 {
        let observed = adapter()
            .observe_order(&device, &order(OrderKind::Sell))
            .await;
        seen.push(
            observed
                .map(|state| state.raw_label)
                .map_err(|e| e.category),
        );
    }

    assert_eq!(
        seen,
        [
            Err(FailureCategory::Relay),
            Err(FailureCategory::Relay),
            Ok("rated".to_owned()),
            Err(FailureCategory::Relay),
            Err(FailureCategory::Relay),
        ]
    );
}

#[tokio::test]
async fn a_closed_order_reads_closed_from_its_status_alone() {
    let (cli, session) = session();
    cli.ok("order.info", reported("success"));
    cli.ok("order.info", reported("fiat-sent"));
    cli.refuse("order.cancel", "not_allowed_by_status");
    cli.ok("order.info", reported("success"));
    cli.always(
        "message.list",
        json!({"messages": [{"action": "rate"}], "truncated": true}),
    );
    let device = device(&session, Actor::Alice);

    let closed = adapter()
        .cancel_if_open(&device, &order(OrderKind::Sell))
        .await
        .unwrap();
    let refused = adapter()
        .cancel_if_open(&device, &order(OrderKind::Sell))
        .await
        .unwrap();

    assert_eq!(
        [closed, refused],
        [CancelOutcome::AlreadyClosed, CancelOutcome::AlreadyClosed]
    );
    assert_eq!(
        cli.commands(),
        ["order.info", "order.info", "order.cancel", "order.info"]
    );
}

#[tokio::test]
async fn a_row_a_truncated_list_cannot_judge_stays_listed_and_reads_inconclusive() {
    let (cli, session) = session();
    let truncated = json!({"messages": [], "truncated": true});
    cli.ok("order.sync", json!({"adopted": []}));
    cli.ok(
        "order.mine",
        json!({"orders": [{"id": "o1"}, {"id": "old"}]}),
    );
    cli.ok(
        "order.info",
        json!({"orders": [{"id": "o1", "status": "active"}], "not_returned": ["old"]}),
    );
    cli.ok("message.list", truncated.clone());
    cli.ok("order.sync", json!({"adopted": []}));
    cli.ok("order.mine", json!({"orders": [{"id": "old"}]}));
    cli.ok("order.info", json!({"orders": [], "not_returned": ["old"]}));
    cli.ok("message.list", truncated.clone());
    cli.ok("order.info", json!({"orders": [], "not_returned": ["old"]}));
    cli.ok("message.list", truncated);
    let adapter = adapter();
    let device = device(&session, Actor::Alice);

    let owned = adapter.owned_orders(&device).await.unwrap();
    let listed = adapter
        .observe_listed_order(&device, &OrderReference::new("old"))
        .await
        .unwrap_err();

    assert_eq!(owned, ["o1", "old"]);
    assert_eq!(listed.category, FailureCategory::Relay);
}

#[tokio::test]
async fn a_take_parked_on_its_bond_is_not_read_from_a_truncated_list() {
    let (cli, session) = session();
    let bond = json!({"action": "pay-bond-invoice", "next_step": "pay_bond_invoice",
        "payload": {"kind": "payment_request", "invoice": "not-a-bolt11", "amount": null, "order": null}});
    cli.ok("order.info", json!({"orders": [], "not_returned": ["o1"]}));
    cli.ok(
        "message.list",
        json!({"messages": [bond], "truncated": true}),
    );

    let error = adapter()
        .observe_order(&device(&session, Actor::Bob), &order(OrderKind::Sell))
        .await
        .unwrap_err();

    assert_eq!(error.category, FailureCategory::Relay);
}
