#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::Arc;

use mortsom_app_contract::{FiatAmount, OrderKind, OrderState};
use mortsom_cli_driver::testing::ScriptedCli;
use mortsom_cli_driver::{CliClient, CliConfig, CliProcess, CliSession, NodeTarget};
use mortsom_common::{Actor, FailureCategory, Lane};
use mortsom_ui_driver::UiSession;
use serde_json::json;

use super::*;

const PK: &str = "dbe0b1be7aafd3cfba92d7463edbd4e33b2969f61bd554d37ac56f032e13355a";

fn every_command() -> BTreeSet<String> {
    CAPABILITY_COMMANDS
        .iter()
        .flat_map(|(_, commands)| commands.iter().map(|c| (*c).to_owned()))
        .collect()
}

fn adapter() -> CliAdapter {
    CliAdapter::new(CliAdapterConfig {
        order_expiry_secs: Some(90),
        commands: every_command(),
        order_expiry_in_seconds: true,
        message_window: MessageWindow::Run,
    })
    .with_poll(Duration::from_millis(1))
}

fn session() -> (Arc<ScriptedCli>, CliSession) {
    let cli = Arc::new(ScriptedCli::default());
    let session = session_over(cli.clone());
    (cli, session)
}

/// A fresh session whose CLI runs on `process`.
fn session_over(process: Arc<dyn CliProcess>) -> CliSession {
    let config = CliConfig {
        binary: "/opt/mostro-cli".into(),
        node: NodeTarget {
            mostro_pubkey: PK.into(),
            relays: vec!["ws://127.0.0.1:7500".into()],
            reply_timeout_secs: None,
        },
        timeout: Duration::from_secs(5),
        path_env: None,
    };
    let client = CliClient::new(config, process);
    client.start_fresh().unwrap();
    CliSession::new(Arc::new(client))
}

fn device(ui: &dyn UiSession, actor: Actor) -> DeviceContext<'_> {
    DeviceContext {
        actor,
        ui,
        ui_timeout: Duration::from_millis(200),
    }
}

fn env() -> TestEnvironment {
    TestEnvironment {
        mostro_pubkey: PK.into(),
        relay_url: "ws://127.0.0.1:7500".into(),
        lane: Lane::Local,
    }
}

fn order(kind: OrderKind) -> OrderReference {
    OrderReference::with_kind("o1", kind)
}

#[test]
fn capabilities_follow_the_commands_the_cli_offers() {
    let full = adapter().capabilities();
    let mut commands = every_command();
    commands.remove("order.rate");
    let partial = CliAdapter::new(CliAdapterConfig {
        commands,
        ..CliAdapterConfig::default()
    })
    .capabilities();

    assert!(full.supports(Capability::Rating));
    assert!(full.supports(Capability::OrderExpiry));
    assert!(!partial.supports(Capability::Rating));
    assert!(!partial.supports(Capability::OrderExpiry));
    for absent in [
        Capability::Nwc,
        Capability::HoldInvoicePayment,
        Capability::OfferedActions,
        Capability::Notifications,
    ] {
        assert!(!full.supports(absent), "{absent}");
    }
    assert_eq!(adapter().profile().name, "cli");
}

#[tokio::test]
async fn onboarding_generates_the_identity_and_a_relaunch_reads_it() {
    let (cli, session) = session();
    cli.ok("identity.init", json!({"identity_pubkey": PK}));
    cli.ok(
        "identity.show",
        json!({"identity_pubkey": PK, "last_trade_index": "0"}),
    );
    let device = device(&session, Actor::Alice);

    let created = adapter().onboard(&device, &env()).await.unwrap();
    let shown = adapter().displayed_identity(&device).await.unwrap();

    assert_eq!(created, shown);
    assert_eq!(cli.commands(), ["identity.init", "identity.show"]);
}

#[tokio::test]
async fn a_malformed_identity_breaks_the_contract() {
    let (cli, session) = session();
    cli.ok("identity.init", json!({"identity_pubkey": "nope"}));

    let error = adapter()
        .onboard(&device(&session, Actor::Alice), &env())
        .await
        .unwrap_err();

    assert_eq!(error.category, FailureCategory::AutomationContract);
}

#[tokio::test]
async fn the_node_check_refuses_another_daemons_book() {
    let (cli, session) = session();
    cli.ok("order.list", json!({"node_pubkey": PK, "orders": []}));
    cli.ok(
        "order.list",
        json!({"node_pubkey": "ab".repeat(32), "orders": []}),
    );
    let device = device(&session, Actor::Bob);

    adapter()
        .configure_node_and_relay(&device, &env())
        .await
        .unwrap();
    let error = adapter()
        .configure_node_and_relay(&device, &env())
        .await
        .unwrap_err();

    assert_eq!(error.category, FailureCategory::EnvironmentMisconfiguration);
}

#[tokio::test]
async fn a_created_order_shows_its_id_and_state() {
    let (cli, session) = session();
    cli.ok(
        "order.new",
        json!({"next_step": "wait_for_taker", "order": {"id": "o1", "status": "pending"}, "payment_request": null}),
    );
    let draft = OrderDraft::fixed(OrderKind::Sell, 10_000, "USD", 5, "cash");

    let visible = adapter()
        .create_order(&device(&session, Actor::Alice), &draft)
        .await
        .unwrap();

    assert_eq!(visible.id.as_deref(), Some("o1"));
    assert_eq!(visible.state, OrderState::Pending);
    let args = &cli.seen()[0].args;
    assert!(args.windows(2).any(|w| w == ["--expires-in", "90"]));
}

#[tokio::test]
async fn a_take_sell_reply_answers_the_payout_amount_without_another_read() {
    let (cli, session) = session();
    cli.ok(
        "order.take-sell",
        json!({"next_step": "add_invoice", "order": {"id": "o1", "sats_amount": "9900"}, "payment_request": null}),
    );
    let adapter = adapter();
    let device = device(&session, Actor::Bob);

    adapter
        .find_and_take_order(&device, &order(OrderKind::Sell))
        .await
        .unwrap();
    let sats = adapter
        .buyer_invoice_amount(&device, &order(OrderKind::Sell))
        .await
        .unwrap();

    assert_eq!(sats, 9900);
    assert_eq!(cli.commands(), ["order.take-sell"]);
}

#[tokio::test]
async fn a_maker_waits_for_the_payout_request_on_its_messages() {
    let (cli, session) = session();
    cli.ok("message.list", json!({"messages": [], "truncated": false}));
    cli.always(
        "message.list",
        json!({"messages": [{"action": "add-invoice", "next_step": "add_invoice", "created_at_unix": "100",
            "payload": {"kind": "order", "order": {"sats_amount": "4950"}}}], "truncated": false}),
    );

    let sats = adapter()
        .buyer_invoice_amount(&device(&session, Actor::Bob), &order(OrderKind::Buy))
        .await
        .unwrap();

    assert_eq!(sats, 4950);
    assert_eq!(cli.commands(), ["message.list", "message.list"]);
}

#[tokio::test]
async fn the_hold_invoice_of_a_take_buy_is_handed_out_for_the_node_to_pay() {
    let (cli, session) = session();
    cli.ok(
        "order.take-buy",
        json!({"next_step": "pay_hold_invoice", "payment_request": "lnbcrt1hold", "order": null}),
    );
    let adapter = adapter();
    let device = device(&session, Actor::Alice);

    adapter
        .find_and_take_order(&device, &order(OrderKind::Buy))
        .await
        .unwrap();
    let handoff = adapter
        .pay_hold_invoice(&device, &order(OrderKind::Buy), PaymentPath::ExternalWallet)
        .await
        .unwrap();
    let nwc = adapter
        .pay_hold_invoice(&device, &order(OrderKind::Buy), PaymentPath::Nwc)
        .await;

    assert_eq!(handoff.kind, PaymentPath::ExternalWallet);
    assert_eq!(handoff.invoice.unwrap().expose(), "lnbcrt1hold");
    assert!(nwc.is_err());
}

#[tokio::test]
async fn a_payout_invoice_travels_on_standard_input_and_a_new_request_must_be_newer() {
    let (cli, session) = session();
    cli.ok(
        "order.add-invoice",
        json!({"result": "waiting_seller_payment"}),
    );
    let invoice = Secret::new("lnbcrt1payout");
    let adapter = adapter();
    let device = device(&session, Actor::Bob);

    let returned = adapter
        .provide_buyer_invoice(&device, &order(OrderKind::Sell), Some(&invoice))
        .await
        .unwrap();
    let without = adapter
        .provide_buyer_invoice(&device, &order(OrderKind::Sell), None)
        .await;

    let seen = &cli.seen()[0];
    assert_eq!(seen.stdin.as_deref(), Some("lnbcrt1payout"));
    assert!(seen.args.windows(2).any(|w| w == ["--invoice", "-"]));
    assert_eq!(returned.unwrap().expose(), "lnbcrt1payout");
    assert!(without.is_err());
    let after = adapter.memo.get(Actor::Bob, "o1").payout_submitted_at;
    assert!(after.is_some());
}

#[tokio::test]
async fn the_state_comes_from_the_nodes_report() {
    let (cli, session) = session();
    cli.ok(
        "order.info",
        json!({"orders": [{"id": "o1", "status": "fiat-sent"}], "not_returned": []}),
    );
    cli.ok("order.info", json!({"orders": [], "not_returned": ["o1"]}));
    cli.ok("message.list", json!({"messages": [], "truncated": false}));
    let device = device(&session, Actor::Alice);

    let state = adapter()
        .observe_order(&device, &order(OrderKind::Sell))
        .await
        .unwrap();
    let missing = adapter()
        .observe_order(&device, &order(OrderKind::Sell))
        .await
        .unwrap_err();

    assert_eq!(state.state, OrderState::FiatSent);
    assert_eq!(state.raw_label, "fiat-sent");
    assert_eq!(missing.category, FailureCategory::Product);
}

#[tokio::test]
async fn a_completed_trade_reads_its_rating_as_the_app_labels_it() {
    let (cli, session) = session();
    for messages in [
        json!({"messages": [{"action": "purchase-completed"}], "truncated": false}),
        json!({"messages": [{"action": "purchase-completed"}, {"action": "rate"}], "truncated": false}),
        json!({"messages": [{"action": "rate"}, {"action": "rate-received"}], "truncated": false}),
    ] {
        cli.ok(
            "order.info",
            json!({"orders": [{"id": "o1", "status": "success"}]}),
        );
        cli.ok("message.list", messages);
    }
    let device = device(&session, Actor::Bob);
    let mut labels = Vec::new();

    for _ in 0..3 {
        let state = adapter()
            .observe_order(&device, &order(OrderKind::Sell))
            .await
            .unwrap();
        assert_eq!(state.state, OrderState::Completed);
        labels.push(state.raw_label);
    }

    assert_eq!(labels, ["success", "pending-rating", "rated"]);
}

#[tokio::test]
async fn a_new_payout_request_reads_waiting_invoice_while_the_escrow_is_settled() {
    let (cli, session) = session();
    let failed = json!({"action": "payment-failed", "next_step": null});
    let asked = json!({"action": "add-invoice", "next_step": "add_invoice"});
    for messages in [
        json!({"messages": [failed.clone()], "truncated": false}),
        json!({"messages": [failed, asked], "truncated": false}),
    ] {
        cli.ok(
            "order.info",
            json!({"orders": [{"id": "o1", "status": "settled-hold-invoice"}]}),
        );
        cli.ok("message.list", messages);
    }
    let device = device(&session, Actor::Bob);

    let before = adapter()
        .observe_order(&device, &order(OrderKind::Sell))
        .await
        .unwrap();
    let after = adapter()
        .observe_order(&device, &order(OrderKind::Sell))
        .await
        .unwrap();

    assert_eq!(before.raw_label, "settled-hold-invoice");
    assert_eq!(after.raw_label, "waiting-invoice");
    assert_eq!(after.state, OrderState::PayoutPending);
}

#[tokio::test]
async fn a_take_parked_on_its_bond_reads_waiting_bond_until_it_is_cancelled() {
    let (cli, session) = session();
    let bond = json!({"action": "pay-bond-invoice", "next_step": "pay_bond_invoice",
        "payload": {"kind": "payment_request", "invoice": "not-a-bolt11", "amount": null, "order": null}});
    let canceled = json!({"action": "canceled", "next_step": null});
    cli.ok("order.info", json!({"orders": [], "not_returned": ["o1"]}));
    cli.ok(
        "message.list",
        json!({"messages": [bond.clone()], "truncated": false}),
    );
    cli.ok("order.info", json!({"orders": [], "not_returned": ["o1"]}));
    cli.ok(
        "message.list",
        json!({"messages": [bond, canceled], "truncated": false}),
    );
    let device = device(&session, Actor::Bob);

    let parked = adapter()
        .observe_order(&device, &order(OrderKind::Sell))
        .await
        .unwrap();
    let gone = adapter()
        .observe_order(&device, &order(OrderKind::Sell))
        .await;

    assert_eq!(
        (parked.state, parked.raw_label.as_str()),
        (OrderState::Taken, "waiting-bond")
    );
    assert_eq!(gone.unwrap_err().category, FailureCategory::Product);
}

#[tokio::test]
async fn a_take_parked_on_its_bond_leaves_the_order_pending_for_the_maker() {
    let (cli, session) = session();
    cli.ok(
        "order.info",
        json!({"orders": [{"id": "o1", "status": "waiting-taker-bond"}]}),
    );

    let state = adapter()
        .observe_order(&device(&session, Actor::Alice), &order(OrderKind::Sell))
        .await
        .unwrap();

    assert_eq!(
        (state.state, state.raw_label.as_str()),
        (OrderState::Pending, "pending")
    );
}

#[tokio::test]
async fn a_closed_order_is_not_cancelled_again() {
    let (cli, session) = session();
    cli.ok(
        "order.info",
        json!({"orders": [{"id": "o1", "status": "canceled"}]}),
    );

    let outcome = adapter()
        .cancel_if_open(&device(&session, Actor::Alice), &order(OrderKind::Sell))
        .await
        .unwrap();

    assert_eq!(outcome, CancelOutcome::AlreadyClosed);
    assert_eq!(cli.commands(), ["order.info"]);
}

#[tokio::test]
async fn a_cancel_refused_as_closed_reads_closed_and_an_unknown_order_is_not_found() {
    let (cli, session) = session();
    cli.ok(
        "order.info",
        json!({"orders": [{"id": "o1", "status": "pending"}]}),
    );
    cli.refuse("order.cancel", "order_already_canceled");
    cli.ok(
        "order.info",
        json!({"orders": [{"id": "o1", "status": "canceled"}]}),
    );
    cli.ok(
        "order.info",
        json!({"orders": [{"id": "o1", "status": "pending"}]}),
    );
    cli.fail("order.cancel", "error", "order_unknown");
    let device = device(&session, Actor::Alice);

    let closed = adapter()
        .cancel_if_open(&device, &order(OrderKind::Sell))
        .await
        .unwrap();
    let unknown = adapter()
        .cancel_if_open(&device, &order(OrderKind::Sell))
        .await
        .unwrap();

    assert_eq!(closed, CancelOutcome::AlreadyClosed);
    assert_eq!(unknown, CancelOutcome::NotFound);
}

#[tokio::test]
async fn a_repeated_cooperative_cancel_stands_and_a_strangers_does_not() {
    let (cli, session) = session();
    for messages in [
        json!({"messages": [{"action": "cooperative-cancel-initiated-by-you"}], "truncated": false}),
        json!({"messages": [{"action": "buyer-took-order"}], "truncated": false}),
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
    assert_eq!(stranger.unwrap_err().category, FailureCategory::Product);
}

#[tokio::test]
async fn own_orders_come_from_the_local_store_after_adopting_announced_ones() {
    let (cli, session) = session();
    cli.ok("order.sync", json!({"adopted": []}));
    cli.ok(
        "order.mine",
        json!({"orders": [{"id": "o1"}, {"id": "o2"}, {"id": "undone"}]}),
    );
    cli.ok(
        "order.info",
        json!({"orders": [{"id": "o1", "status": "pending"}, {"id": "o2", "status": "active"}],
            "not_returned": ["undone"]}),
    );
    cli.ok("message.list", json!({"messages": [], "truncated": false}));
    cli.ok("order.mine", json!({"orders": [{"id": "o1"}]}));
    cli.ok(
        "order.info",
        json!({"orders": [{"id": "o1", "status": "pending"}]}),
    );
    let without_sync = CliAdapter::new(CliAdapterConfig {
        commands: every_command()
            .into_iter()
            .filter(|c| c != "order.sync")
            .collect(),
        ..CliAdapterConfig::default()
    });

    let owned = adapter()
        .owned_orders(&device(&session, Actor::Alice))
        .await
        .unwrap();
    let plain = without_sync
        .owned_orders(&device(&session, Actor::Alice))
        .await
        .unwrap();

    assert_eq!(owned, ["o1", "o2"]);
    assert_eq!(plain, ["o1"]);
    assert_eq!(
        cli.commands(),
        [
            "order.sync",
            "order.mine",
            "order.info",
            "message.list",
            "order.mine",
            "order.info"
        ]
    );
    let info = &cli.seen()[2].args;
    assert_eq!(info.iter().filter(|a| *a == "--order").count(), 3);
}

#[tokio::test]
async fn a_screen_session_is_not_a_cli() {
    let ui = mortsom_ui_driver::ui::tests_support::ScriptedUi::default();

    let error = adapter()
        .displayed_identity(&device(&ui, Actor::Alice))
        .await
        .unwrap_err();

    assert_eq!(error.category, FailureCategory::EnvironmentMisconfiguration);
}

#[tokio::test]
async fn range_takes_name_the_fiat_amount() {
    let (cli, session) = session();
    cli.ok(
        "order.take-sell",
        json!({"next_step": "add_invoice", "order": {"sats_amount": "20000"}}),
    );
    let mut reference = order(OrderKind::Sell);
    reference.priced_by_daemon = true;

    adapter()
        .find_and_take_order_at(&device(&session, Actor::Bob), &reference, 10)
        .await
        .unwrap();

    assert!(
        cli.seen()[0]
            .args
            .windows(2)
            .any(|w| w == ["--amount", "10"])
    );
    let _ = FiatAmount::Exact(1);
}

fn has(args: &[String], flag: &str, value: &str) -> bool {
    args.windows(2).any(|w| w[0] == flag && w[1] == value)
}

#[tokio::test]
async fn trade_steps_rating_and_disputes_are_confirmed_commands() {
    let (cli, session) = session();
    for command in [
        "order.fiat-sent",
        "order.release",
        "order.rate",
        "dispute.open",
    ] {
        cli.ok(command, json!({"order_id": "o1"}));
    }
    let adapter = adapter();
    let device = device(&session, Actor::Bob);
    let o1 = order(OrderKind::Sell);

    adapter.confirm_fiat_sent(&device, &o1).await.unwrap();
    adapter.release(&device, &o1).await.unwrap();
    adapter.rate_counterpart(&device, &o1, 5).await.unwrap();
    adapter.open_dispute(&device, &o1).await.unwrap();
    let out_of_range = adapter.rate_counterpart(&device, &o1, 6).await;

    assert_eq!(
        cli.commands(),
        [
            "order.fiat-sent",
            "order.release",
            "order.rate",
            "dispute.open"
        ]
    );
    for seen in cli.seen() {
        assert!(has(&seen.args, "--order", "o1"), "{:?}", seen.args);
        assert!(seen.args.contains(&"--yes".to_owned()));
    }
    assert!(has(&cli.seen()[2].args, "--rating", "5"));
    assert!(out_of_range.is_err());
    assert_eq!(cli.seen().len(), 4);
}

#[tokio::test]
async fn a_refused_trade_step_keeps_its_category() {
    let (cli, session) = session();
    cli.fail("order.release", "error", "node_refused");

    let error = adapter()
        .release(&device(&session, Actor::Alice), &order(OrderKind::Sell))
        .await
        .unwrap_err();

    assert_eq!(error.category, FailureCategory::Product);
}

#[tokio::test]
async fn cancelling_instead_of_paying_cancels_the_order() {
    let (cli, session) = session();
    cli.ok("order.cancel", json!({"result": "canceled"}));

    adapter()
        .cancel_payment(&device(&session, Actor::Bob), &order(OrderKind::Sell))
        .await
        .unwrap();

    assert_eq!(cli.commands(), ["order.cancel"]);
}

#[tokio::test]
async fn the_cli_has_no_wallet_and_shows_no_actions() {
    let (_, session) = session();
    let device = device(&session, Actor::Alice);

    let nwc = adapter()
        .configure_nwc(&device, "alice", &Secret::new("nostr+walletconnect://x"))
        .await;
    let actions = adapter()
        .visible_actions(&device, &order(OrderKind::Sell))
        .await;
    let existing = adapter().existing_identity(&device).await;

    assert_eq!(
        nwc.unwrap_err().category,
        FailureCategory::AutomationContract
    );
    assert_eq!(
        actions.unwrap_err().category,
        FailureCategory::AutomationContract
    );
    assert!(existing.is_err());
}

#[tokio::test]
async fn an_invoice_the_cli_or_the_node_refuses_is_an_answer() {
    let (cli, session) = session();
    cli.refuse("order.add-invoice", "invalid_invoice");
    cli.ok(
        "order.add-invoice",
        json!({"result": "waiting_seller_payment"}),
    );
    let adapter = adapter();
    let device = device(&session, Actor::Bob);
    let invoice = Secret::new("lnbcrt1short");

    let refused = adapter
        .submit_buyer_invoice(&device, &order(OrderKind::Sell), &invoice)
        .await
        .unwrap();
    let accepted = adapter
        .submit_buyer_invoice(&device, &order(OrderKind::Sell), &invoice)
        .await
        .unwrap();

    assert_eq!(
        refused,
        InvoiceSubmission::Rejected {
            reason: "invalid_invoice".into()
        }
    );
    assert_eq!(accepted, InvoiceSubmission::Accepted { check: None });
}

#[path = "tests_messages.rs"]
mod message_list;
