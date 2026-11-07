//! Anti-abuse bonds over the CLI: the bond invoice a take's or an order's
//! reply or the relay carries, walking away from it, the claim of a
//! slashed bond's share and the slash notice.
use super::*;

#[test]
fn bonds_are_claimed_only_when_the_cli_offers_bond_add_invoice() {
    let mut commands = every_command();
    commands.remove("bond.add-invoice");
    let without = CliAdapter::new(CliAdapterConfig {
        commands,
        ..CliAdapterConfig::default()
    })
    .capabilities();

    assert!(adapter().capabilities().supports(Capability::Bond));
    assert!(!without.supports(Capability::Bond));
}

#[tokio::test]
async fn a_bond_from_a_take_is_handed_out_and_one_on_the_relay_is_waited_for() {
    let (cli, session) = session();
    cli.ok(
        "order.take-sell",
        json!({"next_step": "pay_bond_invoice", "payment_request": "lnbcrt1takerbond"}),
    );
    cli.ok(
        "message.list",
        json!({"messages": [{"action": "pay-bond-invoice", "next_step": "pay_bond_invoice",
            "created_at_unix": "5", "payload": {"kind": "payment_request", "invoice": "lnbcrt1makerbond",
            "amount": null, "order": null}}], "truncated": false}),
    );
    let adapter = adapter();
    let bob = device(&session, Actor::Bob);
    let alice = device(&session, Actor::Alice);

    adapter
        .find_and_take_order(&bob, &order(OrderKind::Sell))
        .await
        .unwrap();
    let taker = adapter
        .pay_bond_invoice(&bob, &order(OrderKind::Sell), PaymentPath::ExternalWallet)
        .await
        .unwrap();
    let maker = adapter
        .pay_bond_invoice(&alice, &order(OrderKind::Sell), PaymentPath::ExternalWallet)
        .await
        .unwrap();

    assert_eq!(taker.invoice.unwrap().expose(), "lnbcrt1takerbond");
    assert_eq!(maker.invoice.unwrap().expose(), "lnbcrt1makerbond");
}

#[tokio::test]
async fn a_bond_from_an_orders_reply_is_handed_out_to_an_external_wallet_only() {
    let (cli, session) = session();
    cli.ok(
        "order.new",
        json!({"next_step": "pay_bond_invoice", "order": {"id": "o1", "status": "waiting-maker-bond"},
            "payment_request": "lnbcrt1makerbond"}),
    );
    let adapter = adapter();
    let alice = device(&session, Actor::Alice);
    let draft = OrderDraft::fixed(OrderKind::Sell, 10_000, "USD", 5, "cash");

    adapter.create_order(&alice, &draft).await.unwrap();
    let nwc = adapter
        .pay_bond_invoice(&alice, &order(OrderKind::Sell), PaymentPath::Nwc)
        .await
        .unwrap_err();
    let handoff = adapter
        .pay_bond_invoice(&alice, &order(OrderKind::Sell), PaymentPath::ExternalWallet)
        .await
        .unwrap();

    assert_eq!(nwc.category, FailureCategory::AutomationContract);
    assert_eq!(handoff.kind, PaymentPath::ExternalWallet);
    assert_eq!(handoff.invoice.unwrap().expose(), "lnbcrt1makerbond");
    assert_eq!(cli.commands(), ["order.new"]);
}

#[tokio::test]
async fn walking_away_from_a_bond_cancels() {
    let (cli, session) = session();
    cli.ok("order.cancel", json!({"result": "canceled"}));
    cli.ok("order.cancel", json!({"result": "canceled"}));
    let device = device(&session, Actor::Bob);

    adapter()
        .decline_bond(&device, &order(OrderKind::Sell))
        .await
        .unwrap();
    adapter()
        .cancel_payment(&device, &order(OrderKind::Sell))
        .await
        .unwrap();

    assert_eq!(cli.commands(), ["order.cancel", "order.cancel"]);
}

#[tokio::test]
async fn a_slashed_bonds_share_is_claimed_and_reads_completed_once_paid() {
    let (cli, session) = session();
    let request = json!({"action": "add-bond-invoice", "next_step": "add_bond_invoice",
        "created_at_unix": "100", "payload": {"kind": "bond_payout_request",
        "order": {"sats_amount": "400"}, "slashed_at": null, "slashed_at_unix": null}});
    let paid = json!({"action": "bond-payout-completed", "created_at_unix": "103",
        "payload": {"kind": "order", "order": {"sats_amount": "400"}}});
    cli.ok("message.list", json!({"messages": [], "truncated": false}));
    cli.ok(
        "message.list",
        json!({"messages": [request.clone()], "truncated": false}),
    );
    cli.ok("bond.add-invoice", json!({"order_id": "o1"}));
    cli.ok(
        "message.list",
        json!({"messages": [request, paid], "truncated": false}),
    );
    let adapter = adapter();
    let device = device(&session, Actor::Alice);
    let o1 = order(OrderKind::Sell);

    let none = adapter.bond_claim(&device, &o1).await.unwrap();
    let pending = adapter.bond_claim(&device, &o1).await.unwrap().unwrap();
    let phase = adapter
        .submit_bond_claim_invoice(&device, &o1, &Secret::new("lnbcrt1share"))
        .await
        .unwrap();

    assert!(none.is_none());
    assert_eq!(
        (pending.amount_sats, pending.status.as_str()),
        (400, "pending")
    );
    assert_eq!(phase, "completed");
    let claim = cli
        .seen()
        .into_iter()
        .find(|r| r.command == "bond.add-invoice")
        .unwrap();
    assert_eq!(claim.stdin.as_deref(), Some("lnbcrt1share"));
}

#[tokio::test]
async fn a_slash_notice_reads_its_cause_from_the_orders_history() {
    let (cli, session) = session();
    cli.ok(
        "message.list",
        json!({"messages": [{"action": "canceled"}], "truncated": false}),
    );
    cli.ok(
        "message.list",
        json!({"messages": [{"action": "canceled"}, {"action": "bond-slashed"}], "truncated": false}),
    );
    let device = device(&session, Actor::Bob);
    let o1 = order(OrderKind::Sell);

    let before = adapter().bond_slash_notice(&device, &o1).await.unwrap();
    let after = adapter().bond_slash_notice(&device, &o1).await.unwrap();

    assert_eq!(before, None);
    assert_eq!(after.as_deref(), Some("timeout"));
}

#[tokio::test]
async fn a_hand_in_with_no_claim_request_after_it_reads_no_phase() {
    let (cli, session) = session();
    cli.ok("bond.add-invoice", json!({"order_id": "o1"}));
    cli.ok("message.list", json!({"messages": [], "truncated": false}));

    let phase = adapter()
        .submit_bond_claim_invoice(
            &device(&session, Actor::Alice),
            &order(OrderKind::Sell),
            &Secret::new("lnbcrt1share"),
        )
        .await
        .unwrap_err();

    assert_eq!(phase.category, FailureCategory::AutomationContract);
    assert!(phase.message.contains("o1"), "{}", phase.message);
    assert_eq!(cli.commands(), ["bond.add-invoice", "message.list"]);
}
