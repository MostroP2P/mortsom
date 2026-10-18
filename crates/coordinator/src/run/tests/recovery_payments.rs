//! Payment recovery for an order the maker's app no longer lists. App v2
//! drops a trade that never went active from My Trades once it is cancelled,
//! so there is no state left to read on screen: the daemon's own terminal
//! status on the relay decides, and only a cancel or an expiry can.
use super::*;

/// Alice's order, taken by Bob, who handed in a payout invoice; then
/// cancelled before it went active, so Alice's app dropped it.
async fn dropped_attempt(
    book: &[&'static str],
    wallet: Arc<RecoveryWallet>,
) -> (RunSummary, Arc<FileJournal>) {
    use mortsom_scenario_kit::journal::RecoveryJournal;
    let mut fixture = world(false);
    fixture.config.run.settle_timeout_secs = 1;
    let journal = saved_run_for(&fixture, Actor::Alice);
    journal
        .record(RecoveryEvent::PaymentExpected {
            order_id: "old-order".into(),
            wallet: "bob".into(),
            hash: "c".repeat(64),
            incoming: true,
            amount_sats: 100,
            bond: false,
        })
        .unwrap();
    let root = journal.root().to_path_buf();
    drop(journal);
    let journal = Arc::new(FileJournal::open(&root).unwrap());
    let adapter = Arc::new(FakeAdapter::full());
    adapter.set("old-order", OrderState::Cancelled);
    adapter.dropped.lock().unwrap().push("old-order".into());
    let (summary, _) = attempt_with_wallet_and_book(
        &fixture,
        adapter,
        journal.clone(),
        Some(("bob".into(), wallet)),
        book,
    )
    .await;
    (summary, journal)
}

/// The live failure of 2026-09-24 (`20260924T224109Z-cae5bc8a8112`,
/// `taker_cancel_at_taken`): the order was cancelled and the relay showed
/// it `canceled`, yet payment recovery tried to read it on Alice's screen.
#[tokio::test]
async fn a_dropped_never_active_order_the_relay_shows_cancelled_resolves() {
    let wallet = RecoveryWallet::new(PaymentState::Pending, 100, 0, 0);

    let (summary, journal) = dropped_attempt(&["pending", "canceled"], wallet.clone()).await;

    assert!(summary.checks.is_healthy(), "{:?}", summary.checks);
    assert!(journal.snapshot().closed);
    assert_eq!(
        *wallet.calls.lock().unwrap(),
        ["cancel invoice", "lookup", "count incoming=true"],
        "the unused payout invoice is cancelled and proven never paid"
    );
}

/// Without the daemon's cancel on the relay a dropped trade proves nothing.
#[tokio::test]
async fn a_dropped_order_the_relay_never_closes_keeps_recovery_open() {
    let wallet = RecoveryWallet::new(PaymentState::Pending, 100, 0, 0);

    let (summary, journal) = dropped_attempt(&["pending"], wallet).await;

    assert!(payment_failed(&summary), "{:?}", summary.checks);
    assert!(!journal.snapshot().closed);
}

/// A completed trade stays in My Trades, so a dropped one was never
/// completed, whatever the book says: a settled payout is not inferred.
#[tokio::test]
async fn a_dropped_order_is_never_taken_as_completed() {
    let wallet = RecoveryWallet::new(PaymentState::Settled, 100, 0, 1);

    let (summary, journal) = dropped_attempt(&["success"], wallet).await;

    assert!(payment_failed(&summary), "{:?}", summary.checks);
    assert!(!journal.snapshot().closed);
}
