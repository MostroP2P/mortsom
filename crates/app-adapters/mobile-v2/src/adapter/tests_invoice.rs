//! The invoice journeys of a run whose wallet is a node Mortsom holds: the
//! hold invoice handed over for the caller to pay, and the buyer invoice
//! typed into the manual form, with the daemon's verdict on it.
use super::*;

/// A run whose wallet is a node Mortsom holds: the app has none connected,
/// so it shows the hold invoice and waits, and accepts a buyer invoice
/// typed into its manual form.
fn app_without_a_wallet() -> FakeUi {
    let ui = app_with_orders();
    // No connected wallet: the add-invoice screen opens on the manual form
    // and the pay screen offers no auto-pay button.
    ui.on_tap(
        Ids::TRADE_ADD_INVOICE,
        &[
            Ids::INVOICE_TEXT,
            Ids::INVOICE_SUBMIT,
            Ids::INVOICE_ORDER_ID,
        ],
    );
    ui.label(Ids::INVOICE_ORDER_ID, "o1");
    ui.on_tap(Ids::INVOICE_SUBMIT, &[Ids::ORDER_STATUS, Ids::ORDER_ID]);
    ui
}

#[tokio::test]
async fn the_hold_invoice_is_handed_over_without_the_app_paying_it() {
    let ui = app_without_a_wallet();
    let a = adapter();
    a.onboard(&device(&ui), &env()).await.unwrap();
    a.create_order(&device(&ui), &draft()).await.unwrap();
    let order = OrderReference::new("o1");

    let handoff = a
        .pay_hold_invoice(&device(&ui), &order, PaymentPath::ExternalWallet)
        .await
        .unwrap();

    assert_eq!(handoff.kind, PaymentPath::ExternalWallet);
    // The caller pays it, so it has to come back.
    assert!(handoff.invoice.is_some());
    let calls = ui.calls().join("\n");
    assert!(
        !calls.contains("tap pay.nwc"),
        "the app has no wallet to pay with",
    );
    // The invoice is a payable instrument until it settles; it never
    // reaches the call log.
    assert!(!calls.contains("secretinvoice"), "{calls}");
}

/// An empty pay screen means there is nothing to hand over. Returning a
/// handoff with no invoice would have the caller pay nothing and the run
/// wait for a settlement that cannot arrive.
#[tokio::test]
async fn an_empty_pay_screen_is_a_product_failure_not_an_empty_handoff() {
    let ui = app_without_a_wallet();
    let a = adapter();
    a.onboard(&device(&ui), &env()).await.unwrap();
    a.create_order(&device(&ui), &draft()).await.unwrap();
    ui.label(Ids::PAY_INVOICE_TEXT, "   ");

    let err = a
        .pay_hold_invoice(
            &device(&ui),
            &OrderReference::new("o1"),
            PaymentPath::ExternalWallet,
        )
        .await
        .unwrap_err();

    assert_eq!(err.category, mortsom_common::FailureCategory::Product);
    assert!(err.message.contains("nothing to pay"), "{err}");
}

#[tokio::test]
async fn a_supplied_buyer_invoice_is_typed_into_the_manual_form() {
    let ui = app_without_a_wallet();
    let a = adapter();
    a.onboard(&device(&ui), &env()).await.unwrap();
    a.create_order(&device(&ui), &draft()).await.unwrap();
    let supplied = Secret::new("lnbc1000n1mortsominvoice");

    let returned = a
        .provide_buyer_invoice(&device(&ui), &OrderReference::new("o1"), Some(&supplied))
        .await
        .unwrap();

    assert_eq!(
        returned.as_ref().map(|s| s.expose()),
        Some(supplied.expose()),
        "the caller correlates the payment against the invoice it issued",
    );
    let calls = ui.calls().join("\n");
    assert!(calls.contains(&format!(
        "type invoice.text ({} chars)",
        supplied.expose().len()
    )));
    assert!(calls.contains("tap invoice.submit"));
    assert!(!calls.contains("mortsominvoice"), "{calls}");
}

/// A submission with a verdict: the trade detail means accepted; the form
/// kept open with `invoice.error` means refused, and the reason is that
/// readout, never the invoice.
#[tokio::test]
async fn a_submitted_buyer_invoice_reports_the_daemons_verdict() {
    let ui = app_without_a_wallet();
    let a = adapter();
    a.onboard(&device(&ui), &env()).await.unwrap();
    a.create_order(&device(&ui), &draft()).await.unwrap();
    let supplied = Secret::new("lnbc1000n1mortsominvoice");
    let verdict = a
        .submit_buyer_invoice(&device(&ui), &OrderReference::new("o1"), &supplied)
        .await
        .unwrap();
    assert_eq!(verdict, InvoiceSubmission::Accepted);

    let ui = app_without_a_wallet();
    ui.on_tap(Ids::INVOICE_SUBMIT, &[Ids::INVOICE_ERROR]);
    ui.on_tap_hide(Ids::INVOICE_SUBMIT, &[Ids::ORDER_STATUS, Ids::ORDER_ID]);
    ui.label(Ids::INVOICE_ERROR, "The node rejected this invoice.");
    a.onboard(&device(&ui), &env()).await.unwrap();
    a.create_order(&device(&ui), &draft()).await.unwrap();
    let verdict = a
        .submit_buyer_invoice(&device(&ui), &OrderReference::new("o1"), &supplied)
        .await
        .unwrap();
    assert_eq!(
        verdict,
        InvoiceSubmission::Rejected {
            reason: "The node rejected this invoice.".into()
        }
    );
    let calls = ui.calls().join("\n");
    assert_eq!(calls.matches("tap invoice.submit").count(), 1);
    assert!(!calls.contains("mortsominvoice"), "{calls}");
}

#[tokio::test]
async fn buyer_invoice_reads_the_requested_amount_and_reuses_the_open_form() {
    let ui = app_without_a_wallet();
    let a = adapter();
    a.onboard(&device(&ui), &env()).await.unwrap();
    a.create_order(&device(&ui), &draft()).await.unwrap();
    ui.on_tap(
        Ids::TRADE_ADD_INVOICE,
        &[
            Ids::INVOICE_TEXT,
            Ids::INVOICE_SUBMIT,
            Ids::INVOICE_ORDER_ID,
            "invoice.amount",
        ],
    );
    ui.label("invoice.amount", "999");
    let order = OrderReference::new("o1");
    assert_eq!(
        a.buyer_invoice_amount(&device(&ui), &order).await.unwrap(),
        999
    );
    a.provide_buyer_invoice(&device(&ui), &order, Some(&Secret::new("private-invoice")))
        .await
        .unwrap();
    assert_eq!(
        ui.calls()
            .iter()
            .filter(|call| *call == "tap trade.addInvoice")
            .count(),
        1
    );
    assert_eq!(
        ui.calls()
            .iter()
            .filter(|call| *call == "tap invoice.submit")
            .count(),
        1
    );
}

#[tokio::test]
async fn buyer_invoice_refuses_zero_or_non_numeric_amounts_before_submission() {
    for value in ["0", "-1", "999 sats", "1.5", "18446744073709551616"] {
        let ui = app_without_a_wallet();
        let a = adapter();
        a.onboard(&device(&ui), &env()).await.unwrap();
        a.create_order(&device(&ui), &draft()).await.unwrap();
        ui.on_tap(
            Ids::TRADE_ADD_INVOICE,
            &[
                Ids::INVOICE_TEXT,
                Ids::INVOICE_SUBMIT,
                Ids::INVOICE_ORDER_ID,
                "invoice.amount",
            ],
        );
        ui.label("invoice.amount", value);
        assert!(
            a.buyer_invoice_amount(&device(&ui), &OrderReference::new("o1"))
                .await
                .is_err()
        );
        assert!(!ui.calls().iter().any(|call| call == "tap invoice.submit"));
    }
}

#[tokio::test]
async fn an_ignored_manual_invoice_edit_is_never_submitted() {
    let ui = app_without_a_wallet();
    let a = adapter();
    a.onboard(&device(&ui), &env()).await.unwrap();
    a.create_order(&device(&ui), &draft()).await.unwrap();
    ui.ignore_text(Ids::INVOICE_TEXT);
    let error = a
        .provide_buyer_invoice(
            &device(&ui),
            &OrderReference::new("o1"),
            Some(&Secret::new("private-invoice")),
        )
        .await
        .unwrap_err();
    assert!(!error.message.contains("private-invoice"));
    assert!(!ui.calls().iter().any(|call| call == "tap invoice.submit"));
}

#[tokio::test]
async fn a_manual_invoice_form_for_another_order_is_never_reused() {
    let ui = app_without_a_wallet();
    let a = adapter();
    a.onboard(&device(&ui), &env()).await.unwrap();
    a.create_order(&device(&ui), &draft()).await.unwrap();
    ui.present(&[Ids::INVOICE_TEXT, Ids::INVOICE_ORDER_ID]);
    ui.label(Ids::INVOICE_ORDER_ID, "other-order");
    let error = a
        .provide_buyer_invoice(
            &device(&ui),
            &OrderReference::new("o1"),
            Some(&Secret::new("private-invoice")),
        )
        .await
        .unwrap_err();
    assert!(error.message.contains("different order"));
    assert!(
        !ui.calls()
            .iter()
            .any(|call| call == "tap invoice.submit" || call.starts_with("type invoice.text"))
    );
}

/// A form kept open after a refusal still shows that refusal's
/// `invoice.error`: the new submission's verdict is read only once the old
/// readout has cleared, never the stale one.
#[tokio::test]
async fn a_stale_invoice_error_is_not_read_as_the_new_verdict() {
    let ui = app_without_a_wallet();
    let a = adapter();
    a.onboard(&device(&ui), &env()).await.unwrap();
    a.create_order(&device(&ui), &draft()).await.unwrap();
    // The previous refusal's readout is still on the form; submitting
    // again clears it, and the trade detail is this submission's verdict.
    ui.present(&[Ids::INVOICE_ERROR]);
    ui.label(Ids::INVOICE_ERROR, "The node rejected this invoice.");
    ui.on_tap_hide(Ids::INVOICE_SUBMIT, &[Ids::INVOICE_ERROR]);

    let verdict = a
        .submit_buyer_invoice(
            &device(&ui),
            &OrderReference::new("o1"),
            &Secret::new("lnbc1000n1mortsominvoice"),
        )
        .await
        .unwrap();

    assert_eq!(verdict, InvoiceSubmission::Accepted);
    assert_eq!(
        ui.calls()
            .iter()
            .filter(|call| *call == "tap invoice.submit")
            .count(),
        1
    );
}
