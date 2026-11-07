//! How the adapter reads `message list`. With `data.truncated` a list that
//! may be incomplete proves no absence: a wait keeps polling, and a verdict
//! or a label that rests on an absence is an inconclusive relay failure.
//! The `--since` window is a day during a run; a recovery reads back to the
//! run's start, and beyond the CLI's reach an absence proves nothing either:
//! an order waiting on its payout invoice is left unresolved.
use super::*;

/// An adapter recovering a run that started at `started` (unix seconds).
fn recovering(started: u64) -> CliAdapter {
    CliAdapter::new(CliAdapterConfig {
        order_expiry_secs: Some(90),
        commands: every_command(),
        order_expiry_in_seconds: true,
        message_window: MessageWindow::SinceRunStart { started },
    })
    .with_poll(Duration::from_millis(1))
}

/// `order info` reporting `o1` in `status`.
fn reported(status: &str) -> serde_json::Value {
    json!({"orders": [{"id": "o1", "status": status}]})
}

/// The `--since` of the `message list` the CLI was asked for.
fn since_of(cli: &ScriptedCli) -> Vec<String> {
    let list = cli
        .seen()
        .into_iter()
        .find(|seen| seen.command == "message.list")
        .unwrap();
    list.args
        .windows(2)
        .filter(|w| w[0] == "--since")
        .map(|w| w[1].clone())
        .collect()
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

#[tokio::test]
async fn a_run_reads_a_days_messages() {
    let (cli, session) = session();
    cli.ok("order.info", reported("settled-hold-invoice"));
    cli.ok("message.list", json!({"messages": [], "truncated": false}));

    adapter()
        .observe_order(&device(&session, Actor::Bob), &order(OrderKind::Sell))
        .await
        .unwrap();

    assert_eq!(
        CliAdapterConfig::default().message_window,
        MessageWindow::Run
    );
    assert_eq!(since_of(&cli), ["1440"]);
}

#[tokio::test]
async fn a_payout_beyond_the_window_is_left_unresolved_and_never_cancelled() {
    let (cli, session) = session();
    cli.always("order.info", reported("settled-hold-invoice"));
    let adapter = recovering(crate::messages::unix_now() - 8 * 24 * 3600);

    let observed = adapter
        .observe_order(&device(&session, Actor::Bob), &order(OrderKind::Sell))
        .await
        .unwrap_err();
    let cancel = adapter
        .cancel_if_open(&device(&session, Actor::Alice), &order(OrderKind::Sell))
        .await
        .unwrap_err();

    for error in [&observed, &cancel] {
        assert_eq!(error.category, FailureCategory::EnvironmentMisconfiguration);
    }
    assert!(
        cancel.message.contains("left unresolved"),
        "{}",
        cancel.message
    );
    let commands = cli.commands();
    assert!(
        !commands.iter().any(|c| c == "order.cancel"),
        "{commands:?}"
    );
    assert!(
        !commands.iter().any(|c| c == "message.list"),
        "{commands:?}"
    );
}

#[tokio::test]
async fn a_recovery_within_the_window_reads_from_the_runs_start() {
    let (cli, session) = session();
    cli.ok("order.info", reported("settled-hold-invoice"));
    cli.ok(
        "message.list",
        json!({"messages": [{"action": "add-invoice", "next_step": "add_invoice"}],
            "truncated": false}),
    );

    let state = recovering(crate::messages::unix_now() - 90)
        .observe_order(&device(&session, Actor::Bob), &order(OrderKind::Sell))
        .await
        .unwrap();

    assert_eq!(
        (state.state, state.raw_label.as_str()),
        (OrderState::PayoutPending, "waiting-invoice")
    );
    assert_eq!(since_of(&cli), ["3"]);
}

#[tokio::test]
async fn a_capped_recovery_still_cancels_an_order_that_waits_on_no_invoice() {
    let (cli, session) = session();
    cli.ok("order.info", reported("active"));
    cli.ok("order.cancel", json!({"result": "canceled"}));
    cli.ok("order.info", json!({"orders": [], "not_returned": ["o1"]}));
    cli.ok("order.cancel", json!({"result": "canceled"}));
    let adapter = recovering(0);
    let device = device(&session, Actor::Alice);

    let active = adapter
        .cancel_if_open(&device, &order(OrderKind::Sell))
        .await
        .unwrap();
    let unreported = adapter
        .cancel_if_open(&device, &order(OrderKind::Sell))
        .await
        .unwrap();

    assert_eq!(
        [active, unreported],
        [CancelOutcome::Cancelled, CancelOutcome::Cancelled]
    );
    assert_eq!(
        cli.commands(),
        ["order.info", "order.cancel", "order.info", "order.cancel"]
    );
}

#[tokio::test]
async fn an_unreadable_report_stops_a_cancel_only_beyond_the_window() {
    let (cli, session) = session();
    cli.fail("order.info", "error", "relay_connection_failed");
    cli.ok("order.cancel", json!({"result": "canceled"}));
    cli.fail("order.info", "error", "relay_connection_failed");
    let device = device(&session, Actor::Alice);

    let within = adapter()
        .cancel_if_open(&device, &order(OrderKind::Sell))
        .await
        .unwrap();
    let capped = recovering(0)
        .cancel_if_open(&device, &order(OrderKind::Sell))
        .await;

    assert_eq!(within, CancelOutcome::Cancelled);
    assert_eq!(capped.unwrap_err().category, FailureCategory::Relay);
    assert_eq!(cli.commands(), ["order.info", "order.cancel", "order.info"]);
}

#[tokio::test]
async fn a_capped_recovery_reads_a_week_back_and_proves_no_cancel_request_absent() {
    let (cli, session) = session();
    for messages in [
        json!([{"action": "cooperative-cancel-initiated-by-you"}]),
        json!([{"action": "buyer-took-order"}]),
    ] {
        cli.ok("order.info", reported("active"));
        cli.refuse("order.cancel", "invalid_pubkey");
        cli.ok(
            "message.list",
            json!({"messages": messages, "truncated": false}),
        );
    }
    let adapter = recovering(0);
    let device = device(&session, Actor::Alice);

    let standing = adapter
        .cancel_if_open(&device, &order(OrderKind::Sell))
        .await
        .unwrap();
    let absent = adapter
        .cancel_if_open(&device, &order(OrderKind::Sell))
        .await
        .unwrap_err();

    assert_eq!(standing, CancelOutcome::Cancelled);
    assert_eq!(
        absent.category,
        FailureCategory::EnvironmentMisconfiguration
    );
    assert!(absent.message.contains("10080"), "{}", absent.message);
    assert_eq!(since_of(&cli), ["10080"]);
}

#[tokio::test]
async fn a_bond_wait_keeps_polling_past_a_truncated_list() {
    let (cli, session) = session();
    cli.ok("message.list", json!({"messages": [], "truncated": true}));
    cli.ok(
        "message.list",
        json!({"messages": [{"action": "pay-bond-invoice", "next_step": "pay_bond_invoice",
            "created_at_unix": "100", "payload": {"kind": "payment_request",
            "invoice": "lnbcrt1makerbond", "amount": null, "order": null}}], "truncated": true}),
    );

    let handoff = adapter()
        .pay_bond_invoice(
            &device(&session, Actor::Alice),
            &order(OrderKind::Sell),
            PaymentPath::ExternalWallet,
        )
        .await
        .unwrap();

    assert_eq!(handoff.invoice.unwrap().expose(), "lnbcrt1makerbond");
    assert_eq!(cli.commands(), ["message.list", "message.list"]);
}

#[tokio::test]
async fn bond_reads_on_a_truncated_list_fail_as_relay_failures() {
    let (cli, session) = session();
    let request = json!({"action": "add-bond-invoice", "next_step": "add_bond_invoice",
        "created_at_unix": "100", "payload": {"kind": "bond_payout_request",
        "order": {"sats_amount": "400"}}});
    for messages in [
        json!([]),
        json!([{"action": "canceled"}, {"action": "bond-slashed"}]),
        json!([request]),
    ] {
        cli.ok(
            "message.list",
            json!({"messages": messages, "truncated": true}),
        );
    }
    cli.ok("bond.add-invoice", json!({"order_id": "o1"}));
    let adapter = adapter();
    let device = device(&session, Actor::Alice);
    let o1 = order(OrderKind::Sell);

    let claim = adapter.bond_claim(&device, &o1).await.unwrap_err();
    let notice = adapter.bond_slash_notice(&device, &o1).await.unwrap_err();
    let phase = adapter
        .submit_bond_claim_invoice(&device, &o1, &Secret::new("lnbcrt1share"))
        .await
        .unwrap_err();

    for error in [&claim, &notice, &phase] {
        assert_eq!(error.category, FailureCategory::Relay);
        assert!(error.message.contains("truncated"), "{}", error.message);
    }
    assert_eq!(
        cli.commands(),
        [
            "message.list",
            "message.list",
            "bond.add-invoice",
            "message.list"
        ]
    );
}

#[tokio::test]
async fn bond_reads_beyond_the_window_prove_no_absence() {
    let (cli, session) = session();
    for messages in [
        json!([{"action": "canceled"}, {"action": "bond-slashed"}]),
        json!([{"action": "admin-took-dispute"}, {"action": "bond-slashed"}]),
    ] {
        cli.ok(
            "message.list",
            json!({"messages": messages, "truncated": false}),
        );
    }
    cli.always("message.list", json!({"messages": [], "truncated": false}));
    cli.ok("bond.add-invoice", json!({"order_id": "o1"}));
    let adapter = recovering(0);
    let device = device(&session, Actor::Alice);
    let o1 = order(OrderKind::Sell);

    let timeout = adapter.bond_slash_notice(&device, &o1).await.unwrap_err();
    let dispute = adapter.bond_slash_notice(&device, &o1).await.unwrap();
    let claim = adapter.bond_claim(&device, &o1).await.unwrap_err();
    let phase = adapter
        .submit_bond_claim_invoice(&device, &o1, &Secret::new("lnbcrt1share"))
        .await
        .unwrap_err();
    let bond = adapter
        .pay_bond_invoice(&device, &o1, PaymentPath::ExternalWallet)
        .await
        .unwrap_err();

    assert_eq!(dispute.as_deref(), Some("dispute"));
    for error in [&timeout, &claim, &phase, &bond] {
        assert_eq!(error.category, FailureCategory::EnvironmentMisconfiguration);
        assert!(error.message.contains("10080"), "{}", error.message);
    }
    assert_eq!(since_of(&cli), ["10080"]);
}
