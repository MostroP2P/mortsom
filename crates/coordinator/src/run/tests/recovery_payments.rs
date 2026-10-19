//! Payment recovery for an order the maker's app no longer lists. App v2
//! drops a trade that never went active from My Trades once it is cancelled,
//! so there is no state left to read on screen: the daemon's own terminal
//! status on the relay decides, and only a cancel or an expiry can.
use super::*;

/// One journaled payment on `old-order`: the wallet it is on, whether it
/// comes in, and what that wallet holds for it.
struct Paid {
    wallet: &'static str,
    incoming: bool,
    holds: Arc<RecoveryWallet>,
}

/// The hold payment's hash, apart from the payout's `c…`: the journal never
/// takes one hash for two payments.
const HOLD_HASH: &str = "d";

/// Bob's payout invoice, never paid: recovery cancels it.
fn payout() -> Paid {
    Paid {
        wallet: "bob",
        incoming: true,
        holds: RecoveryWallet::new(PaymentState::Pending, 100, 0, 0),
    }
}

/// Alice's hold payment, returned when the order was cancelled.
fn returned_hold() -> Paid {
    Paid {
        wallet: "alice",
        incoming: false,
        holds: RecoveryWallet::for_hash(&HOLD_HASH.repeat(64), PaymentState::Failed, 100, 0, 0),
    }
}

/// Alice's order, taken by Bob, with `payments` journaled on it; Alice's app
/// shows it `state`, or has dropped it from My Trades when `dropped`.
async fn attempt_on(
    state: OrderState,
    dropped: bool,
    book: &[&'static str],
    payments: Vec<Paid>,
) -> (RunSummary, Arc<FileJournal>, Arc<FakeAdapter>) {
    attempt_with(FakeAdapter::full(), state, dropped, book, payments).await
}

/// [`attempt_on`], with `adapter` as Alice's and Bob's app.
async fn attempt_with(
    adapter: FakeAdapter,
    state: OrderState,
    dropped: bool,
    book: &[&'static str],
    payments: Vec<Paid>,
) -> (RunSummary, Arc<FileJournal>, Arc<FakeAdapter>) {
    use mortsom_scenario_kit::journal::RecoveryJournal;
    let mut fixture = world(false);
    fixture.config.run.settle_timeout_secs = 1;
    let journal = saved_run_for(&fixture, Actor::Alice);
    for paid in &payments {
        journal
            .record(RecoveryEvent::PaymentExpected {
                order_id: "old-order".into(),
                wallet: paid.wallet.into(),
                hash: paid.holds.hash.clone(),
                incoming: paid.incoming,
                amount_sats: 100,
                bond: false,
            })
            .unwrap();
    }
    let root = journal.root().to_path_buf();
    drop(journal);
    let journal = Arc::new(FileJournal::open(&root).unwrap());
    let adapter = Arc::new(adapter);
    adapter.set("old-order", state);
    if dropped {
        adapter.dropped.lock().unwrap().push("old-order".into());
    }
    let wallets = payments
        .into_iter()
        .map(|paid| (paid.wallet.to_owned(), paid.holds))
        .collect();
    let (summary, _) =
        attempt_with_wallets_and_book(&fixture, adapter.clone(), journal.clone(), wallets, book)
            .await;
    (summary, journal, adapter)
}

/// Alice's order, taken by Bob, who handed in a payout invoice; then
/// cancelled before it went active, so Alice's app dropped it.
async fn dropped_attempt(
    book: &[&'static str],
    wallet: Arc<RecoveryWallet>,
) -> (RunSummary, Arc<FileJournal>) {
    let payout = Paid {
        holds: wallet,
        ..payout()
    };
    let (summary, journal, _) = attempt_on(OrderState::Cancelled, true, book, vec![payout]).await;
    (summary, journal)
}

/// How many times the maker's trades list was read.
fn list_reads(adapter: &FakeAdapter) -> usize {
    adapter
        .recovery_calls
        .lock()
        .unwrap()
        .iter()
        .filter(|call| *call == "owned:alice")
        .count()
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

/// Live, 2026-10-19 (`buyer_maker_bond_slashed_on_timeout`): four payments
/// on one dropped order, and each one tried its row again -- 33 s apiece.
/// An order's terminal state is final: it is read once, from the list, and
/// every payment on it is still settled by its own hash.
#[tokio::test]
async fn a_dropped_orders_state_is_read_once_however_many_payments_it_carries() {
    let book = &["pending", "canceled"][..];
    let (one, one_journal, one_adapter) =
        attempt_on(OrderState::Cancelled, true, book, vec![payout()]).await;
    let (both, both_journal, both_adapter) = attempt_on(
        OrderState::Cancelled,
        true,
        book,
        vec![payout(), returned_hold()],
    )
    .await;

    for (summary, journal) in [(&one, &one_journal), (&both, &both_journal)] {
        assert!(summary.checks.is_healthy(), "{:?}", summary.checks);
        assert!(journal.snapshot().closed);
    }
    assert_eq!(list_reads(&both_adapter), list_reads(&one_adapter));
    assert_eq!(
        *both_adapter.reopened_dropped.lock().unwrap(),
        *one_adapter.reopened_dropped.lock().unwrap(),
        "a second payment opened the dropped row again"
    );
}

/// The same holds for a completed order: both screens are read for the
/// order, not again for each of its payments.
#[tokio::test]
async fn a_completed_orders_screens_are_read_once_however_many_payments_it_carries() {
    let paid_out = || Paid {
        wallet: "bob",
        incoming: true,
        holds: RecoveryWallet::new(PaymentState::Settled, 100, 0, 1),
    };
    let held = || Paid {
        wallet: "alice",
        incoming: false,
        holds: RecoveryWallet::for_hash(&HOLD_HASH.repeat(64), PaymentState::Settled, 100, 0, 1),
    };
    let book = &["canceled", "success"][..];
    let (one, _, one_adapter) =
        attempt_on(OrderState::Completed, false, book, vec![paid_out()]).await;
    let (both, _, both_adapter) =
        attempt_on(OrderState::Completed, false, book, vec![paid_out(), held()]).await;

    assert!(one.checks.is_healthy(), "{:?}", one.checks);
    assert!(both.checks.is_healthy(), "{:?}", both.checks);
    assert_eq!(
        both_adapter.observed_actors.lock().unwrap().len(),
        one_adapter.observed_actors.lock().unwrap().len(),
        "a second payment read the screens again"
    );
}

/// A list that cannot be read proves nothing either way: the order's state
/// is read on its screen, as it was before the list was asked first.
#[tokio::test]
async fn an_unreadable_list_leaves_the_state_to_the_screen() {
    let adapter = FakeAdapter {
        unreadable_list: true,
        ..FakeAdapter::full()
    };
    let paid_out = Paid {
        wallet: "bob",
        incoming: true,
        holds: RecoveryWallet::new(PaymentState::Settled, 100, 0, 1),
    };

    let (summary, _, adapter) = attempt_with(
        adapter,
        OrderState::Completed,
        false,
        &["canceled", "success"],
        vec![paid_out],
    )
    .await;

    assert!(!payment_failed(&summary), "{:?}", summary.checks);
    assert!(
        adapter
            .observed_actors
            .lock()
            .unwrap()
            .contains(&Actor::Alice),
        "the maker's screen was never read"
    );
}
