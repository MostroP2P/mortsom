//! Discovery after a started submission: an order the client lists that the
//! submission did not know is the submission's order, unless the client
//! lists it without proof (the CLI's row a truncated message list cannot
//! judge). Such a row is cleaned up like any other, but the submission stays
//! open for the relay audit or the operator to decide.
use super::*;
use mortsom_scenario_kit::journal::RecoveryJournal;

/// A saved run whose maker, Alice, started a second submission knowing only
/// `old-order`; her client now also lists `stale-row`.
fn second_submission(fixture: &World) -> Arc<FileJournal> {
    let journal = saved_run(fixture);
    journal
        .record(RecoveryEvent::SubmissionStarted {
            actor: Actor::Alice,
            known_orders: vec!["old-order".into()],
        })
        .unwrap();
    journal
}

fn found(journal: &FileJournal, id: &str) -> bool {
    journal
        .snapshot()
        .events
        .iter()
        .any(|event| matches!(event, RecoveryEvent::OrderFound { order, .. } if order.id == id))
}

fn listing(unproven: &[&str]) -> Arc<FakeAdapter> {
    let adapter = FakeAdapter {
        unproven: unproven.iter().map(|id| (*id).to_owned()).collect(),
        ..FakeAdapter::full()
    };
    adapter.set("old-order", OrderState::Pending);
    adapter.set("stale-row", OrderState::Pending);
    Arc::new(adapter)
}

#[tokio::test]
async fn a_row_listed_without_proof_never_resolves_a_submission() {
    let fixture = world(false);
    let journal = second_submission(&fixture);

    let (summary, _) = attempt(&fixture, listing(&["stale-row"]), journal.clone()).await;

    assert!(!found(&journal, "stale-row"));
    assert_eq!(
        recovery_impl::open_submissions(&journal),
        [(Actor::Alice, vec!["old-order".to_owned()])]
    );
    assert!(!journal.snapshot().closed);
    let cleanup = summary.cleanup.as_ref().unwrap();
    assert!(cleanup.entries.iter().any(|e| e.order.id == "stale-row"));
}

/// A row a truncated list keeps that the next, complete list drops was
/// never proven: discovery takes a row and its proof from one read, so the
/// later read's silence about it cannot pass for proof.
#[tokio::test]
async fn a_row_a_later_read_drops_is_never_proven_by_that_read() {
    let fixture = world(false);
    let journal = second_submission(&fixture);
    let adapter = listing(&["stale-row"]);
    *adapter.proof_reads_left.lock().unwrap() = Some(1);

    attempt(&fixture, adapter, journal.clone()).await;

    assert!(!found(&journal, "stale-row"));
    assert_eq!(
        recovery_impl::open_submissions(&journal),
        [(Actor::Alice, vec!["old-order".to_owned()])]
    );
}

#[tokio::test]
async fn a_proven_row_resolves_the_submission_that_did_not_know_it() {
    let fixture = world(false);
    let journal = second_submission(&fixture);

    attempt(&fixture, listing(&[]), journal.clone()).await;

    assert!(found(&journal, "stale-row"));
    assert!(recovery_impl::open_submissions(&journal).is_empty());
}
