//! What the node asked an actor to do next on an order, read with
//! `message list`: the requests that arrive while the actor is idle (a hold
//! invoice for the maker, a payout request after the seller paid, a bond, a
//! slashed bond's share).
//! A list may come back truncated (`data.truncated`): what it holds is read
//! as usual, and what is missing from it proves nothing. Nor does an absence
//! in a recovery's window that falls short of the run's start.

use std::time::Duration;

use mortsom_app_contract::{AdapterError, DeviceContext};
use mortsom_cli_driver::{Call, Envelope, unsigned_of};
use mortsom_common::{FailureCategory, Secret};
use serde_json::Value;
use tokio::time::Instant;

use crate::access::{missing, run};

/// How far back `message list` reads (`--since`, minutes back from each read).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum MessageWindow {
    /// A run's: a day, longer than any scenario.
    #[default]
    Run,
    /// A recovery's: back to the run's start as its journal records it,
    /// never further than mostro-cli reads.
    SinceRunStart {
        /// Unix seconds the run started.
        started: u64,
    },
}

impl MessageWindow {
    /// A run's window, in minutes.
    const RUN_MINUTES: u64 = 1440;
    /// The farthest back mostro-cli reads: `--since` takes 1 to 10080.
    pub(crate) const MAX_MINUTES: u64 = 10_080;
    /// Added to a recovery's window: mostro-cli counts `--since` back from
    /// its own clock, read a moment after the adapter's, and the run's
    /// first seconds must stay inside the window.
    const MARGIN_MINUTES: u64 = 1;

    /// Minutes back to the oldest message a read at `now` (unix seconds)
    /// wants: the time since the run's start rounded up, plus
    /// [`Self::MARGIN_MINUTES`]. A clock behind the run's start (set back,
    /// or a journal written on a host whose clock ran ahead) cannot say how
    /// long ago the run started: it reads as far back as mostro-cli can,
    /// and does not count as falling short.
    fn wanted_at(self, now: u64) -> u64 {
        match self {
            Self::Run => Self::RUN_MINUTES,
            Self::SinceRunStart { started } if now < started => Self::MAX_MINUTES,
            Self::SinceRunStart { started } => (now - started).div_ceil(60) + Self::MARGIN_MINUTES,
        }
    }

    /// The `--since` of a read at `now`, at most [`Self::MAX_MINUTES`].
    pub(crate) fn minutes_at(self, now: u64) -> u64 {
        self.wanted_at(now).min(Self::MAX_MINUTES)
    }

    /// Whether a read at `now` falls short of the run's start: a request
    /// older than the window may exist and cannot be read.
    pub(crate) fn is_capped_at(self, now: u64) -> bool {
        self.wanted_at(now) > Self::MAX_MINUTES
    }

    /// Whether a read now falls short of the run's start.
    pub(crate) fn is_capped(self) -> bool {
        self.is_capped_at(unix_now())
    }
}

/// Unix seconds now, by the host's clock; zero before the epoch.
pub(crate) fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs())
}

/// `next_step` of a hold invoice to pay.
pub(crate) const PAY_HOLD: &str = "pay_hold_invoice";
/// `next_step` of a payout invoice to hand in.
pub(crate) const ADD_INVOICE: &str = "add_invoice";
/// `next_step` of an anti-abuse bond to pay.
pub(crate) const PAY_BOND: &str = "pay_bond_invoice";

/// One request of the node.
#[derive(Debug, Clone)]
pub(crate) struct Request {
    /// The invoice to pay, when the request carries one.
    pub invoice: Option<Secret>,
    /// Sats asked for: the payload's amount, else its order's.
    pub amount_sats: Option<u64>,
    /// Unix seconds the node sent it.
    pub created_at: i64,
}

/// What `message list` returned for one order.
#[derive(Debug)]
pub(crate) struct Inbox {
    /// The node's messages, oldest first.
    pub messages: Vec<Value>,
    /// `data.truncated`: a relay returned its per-relay limit, or more than
    /// the CLI's record cap matched. The list may then be incomplete, so an
    /// action absent from it proves nothing.
    pub truncated: bool,
    /// Whether the read fell short of the run's start (a recovery window
    /// capped at [`MessageWindow::MAX_MINUTES`]): an action older than the
    /// window may exist and cannot be read.
    pub capped: bool,
}

impl Inbox {
    /// Backs a verdict that rests on `what` being absent: `Ok` on a complete
    /// list read back to the run's start; a relay failure on a list that may
    /// be incomplete, and an environment failure on a window that falls
    /// short of the run's start.
    pub(crate) fn prove_absence(&self, order_id: &str, what: &str) -> Result<(), AdapterError> {
        if self.truncated {
            return Err(inconclusive(order_id, what));
        }
        if self.capped {
            return Err(beyond_reach(order_id, what));
        }
        Ok(())
    }
}

/// The relay failure of a verdict that a truncated list cannot back.
pub(crate) fn inconclusive(order_id: &str, what: &str) -> AdapterError {
    AdapterError::new(
        FailureCategory::Relay,
        format!(
            "`message list` for order {order_id} may be incomplete (`truncated`), \
             so the absence of {what} proves nothing"
        ),
    )
}

/// The failure of a verdict that a window short of the run's start cannot
/// back: the run started longer ago than mostro-cli reads.
fn beyond_reach(order_id: &str, what: &str) -> AdapterError {
    AdapterError::environment(format!(
        "the run's start lies beyond the {} minutes `message list` reads back for order \
         {order_id}, so the absence of {what} proves nothing",
        MessageWindow::MAX_MINUTES
    ))
}

/// The messages and the completeness flag of a `message list` outcome (the
/// contract promises both), read through a window that is `capped` or not.
fn inbox_of(outcome: &Envelope, capped: bool) -> Result<Inbox, AdapterError> {
    let messages = outcome
        .field(&["messages"])
        .and_then(Value::as_array)
        .cloned()
        .ok_or_else(|| missing("message.list", "messages"))?;
    let truncated = outcome
        .field(&["truncated"])
        .and_then(Value::as_bool)
        .ok_or_else(|| missing("message.list", "truncated"))?;
    Ok(Inbox {
        messages,
        truncated,
        capped,
    })
}

/// The messages the node sent this actor about `order_id` within
/// `window`, oldest first, and whether the list may be incomplete.
pub(crate) async fn list(
    device: &DeviceContext<'_>,
    order_id: &str,
    window: MessageWindow,
) -> Result<Inbox, AdapterError> {
    let now = unix_now();
    let call = Call::node(&["message", "list"])
        .arg("order", order_id)
        .arg("since", window.minutes_at(now).to_string());
    inbox_of(&run(device, call).await?, window.is_capped_at(now))
}

/// The newest request with `next_step` among `steps` sent after `after`.
pub(crate) fn newest(messages: &[Value], steps: &[&str], after: Option<i64>) -> Option<Request> {
    messages
        .iter()
        .rev()
        .filter_map(|message| request_of(message, steps))
        .find(|request| after.is_none_or(|after| request.created_at > after))
}

fn request_of(message: &Value, steps: &[&str]) -> Option<Request> {
    let step = message.get("next_step")?.as_str()?;
    if !steps.contains(&step) {
        return None;
    }
    let payload = message.get("payload").unwrap_or(&Value::Null);
    let amount = payload
        .get("amount")
        .and_then(unsigned_of)
        .or_else(|| {
            payload
                .get("order")
                .and_then(|order| order.get("sats_amount"))
                .and_then(unsigned_of)
        })
        .filter(|sats| *sats > 0);
    let invoice = payload
        .get("invoice")
        .and_then(Value::as_str)
        .map(Secret::new);
    // The node repeats a step with other payloads (a `pay-invoice` naming
    // the peer, for one): only a message carrying what its step needs is
    // the request itself.
    let complete = match step {
        PAY_HOLD | PAY_BOND => invoice.is_some(),
        _ => amount.is_some(),
    };
    if !complete {
        return None;
    }
    Some(Request {
        invoice,
        amount_sats: amount,
        created_at: message
            .get("created_at_unix")
            .and_then(unsigned_of)
            .and_then(|secs| i64::try_from(secs).ok())
            .unwrap_or(0),
    })
}

/// Waits, within the device's deadline, for a request among `steps` newer
/// than `after`. A truncated list is read again like one without the
/// request; a deadline that passes before any read ends, or after one that
/// was truncated or fell short of the run's start, proves nothing about the
/// node.
pub(crate) async fn wait_for(
    device: &DeviceContext<'_>,
    order_id: &str,
    steps: &[&str],
    after: Option<i64>,
    window: MessageWindow,
    poll: Duration,
) -> Result<Request, AdapterError> {
    let deadline = Instant::now() + device.ui_timeout;
    // The last read that lacked the request.
    let mut last: Option<Inbox> = None;
    let waited = tokio::time::timeout_at(deadline, async {
        loop {
            let inbox = list(device, order_id, window).await?;
            if let Some(request) = newest(&inbox.messages, steps, after) {
                return Ok(request);
            }
            last = Some(inbox);
            tokio::time::sleep(poll).await;
        }
    })
    .await;
    // A read cut short by the deadline is abandoned whole: the CLI driver
    // reaps its child and clears its runtime marker on its own.
    waited.unwrap_or_else(|_| {
        Err(never_asked(
            order_id,
            steps,
            device.ui_timeout,
            last.as_ref(),
        ))
    })
}

/// The failure of a wait for `steps` that ran out after `waited`: the node
/// never asked, unless the `last` read cannot prove it (see
/// [`Inbox::prove_absence`]) and the request may be beyond it. Without a
/// `last` read no read ended in time, so nothing was read to prove it: a
/// relay failure, as for a truncated list.
fn never_asked(
    order_id: &str,
    steps: &[&str],
    waited: Duration,
    last: Option<&Inbox>,
) -> AdapterError {
    let what = steps.join(" or ");
    let Some(inbox) = last else {
        return AdapterError::new(
            FailureCategory::Relay,
            format!(
                "no `message list` read for order {order_id} ended within {waited:?}, \
                 so nothing shows whether the node asked for {what}"
            ),
        );
    };
    let unproven = inbox
        .prove_absence(order_id, &format!("a request for {what} within {waited:?}"))
        .err();
    unproven.unwrap_or_else(|| {
        AdapterError::product(format!(
            "the node did not ask for {what} on order {order_id} within {waited:?}"
        ))
    })
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use serde_json::json;

    fn message(step: &str, at: &str, payload: Value) -> Value {
        json!({"action": "x", "next_step": step, "created_at_unix": at, "payload": payload})
    }

    #[test]
    fn the_newest_matching_request_wins_and_carries_its_invoice() {
        let messages = vec![
            message(
                PAY_HOLD,
                "10",
                json!({"kind": "payment_request", "invoice": "lnbcrt1old", "amount": null, "order": null}),
            ),
            message(
                ADD_INVOICE,
                "11",
                json!({"kind": "order", "order": {"sats_amount": "9900"}}),
            ),
            message(
                PAY_HOLD,
                "12",
                json!({"kind": "payment_request", "invoice": "lnbcrt1new", "amount": "10000", "order": null}),
            ),
        ];

        let hold = newest(&messages, &[PAY_HOLD], None).unwrap();
        let payout = newest(&messages, &[ADD_INVOICE], None).unwrap();

        assert_eq!(hold.invoice.unwrap().expose(), "lnbcrt1new");
        assert_eq!(hold.amount_sats, Some(10000));
        assert_eq!(payout.amount_sats, Some(9900));
        assert_eq!(payout.created_at, 11);
    }

    #[test]
    fn a_repeat_of_the_step_without_the_invoice_is_not_the_request() {
        let messages = vec![
            message(
                PAY_HOLD,
                "10",
                json!({"kind": "payment_request", "invoice": "lnbcrt1hold", "amount": null, "order": null}),
            ),
            message(
                PAY_HOLD,
                "11",
                json!({"kind": "peer", "counterparty_pubkey": "ab"}),
            ),
            message(ADD_INVOICE, "12", json!({"kind": "none"})),
        ];

        let hold = newest(&messages, &[PAY_HOLD], None).unwrap();

        assert_eq!(hold.invoice.unwrap().expose(), "lnbcrt1hold");
        assert!(newest(&messages, &[ADD_INVOICE], None).is_none());
    }

    #[test]
    fn a_request_older_than_the_last_submission_is_ignored() {
        let messages = vec![message(
            ADD_INVOICE,
            "11",
            json!({"kind": "order", "order": {"sats_amount": "9900"}}),
        )];

        assert!(newest(&messages, &[ADD_INVOICE], Some(11)).is_none());
        assert!(newest(&messages, &[ADD_INVOICE], Some(10)).is_some());
        assert!(newest(&messages, &[PAY_BOND], None).is_none());
    }

    fn outcome(data: Value) -> Envelope {
        let output = mortsom_cli_driver::testing::document("message.list", "ok", &data, None, 0);
        Envelope::parse(&output.stdout, output.code).unwrap()
    }

    #[test]
    fn a_listing_says_whether_it_may_be_incomplete() {
        let complete = outcome(json!({"messages": [{"action": "rate"}], "truncated": false}));
        let truncated = outcome(json!({"messages": [], "truncated": true}));

        let complete = inbox_of(&complete, false).unwrap();
        let truncated = inbox_of(&truncated, true).unwrap();

        assert_eq!(complete.messages.len(), 1);
        assert!(!complete.truncated && !complete.capped);
        assert!(truncated.truncated && truncated.capped);
    }

    #[test]
    fn a_listing_without_a_boolean_truncated_flag_breaks_the_contract() {
        let absent = outcome(json!({"messages": []}));
        let text = outcome(json!({"messages": [], "truncated": "true"}));

        for listing in [absent, text] {
            let error = inbox_of(&listing, false).unwrap_err();
            assert_eq!(error.category, FailureCategory::AutomationContract);
        }
    }

    fn inbox(truncated: bool, capped: bool) -> Inbox {
        Inbox {
            messages: vec![],
            truncated,
            capped,
        }
    }

    #[test]
    fn an_absence_stands_only_on_a_complete_list_read_back_to_the_runs_start() {
        let proven = inbox(false, false).prove_absence("o1", "bond-slashed");
        let truncated = inbox(true, false).prove_absence("o1", "bond-slashed");
        let capped = inbox(false, true).prove_absence("o1", "bond-slashed");

        assert!(proven.is_ok());
        let truncated = truncated.unwrap_err();
        let capped = capped.unwrap_err();
        assert_eq!(truncated.category, FailureCategory::Relay);
        assert_eq!(
            capped.category,
            FailureCategory::EnvironmentMisconfiguration
        );
        for error in [&truncated, &capped] {
            assert!(error.message.contains("o1"), "{}", error.message);
            assert!(error.message.contains("bond-slashed"), "{}", error.message);
        }
        assert!(capped.message.contains("10080"), "{}", capped.message);
    }

    #[test]
    fn a_wait_that_ran_out_blames_the_node_only_after_a_complete_read() {
        let waited = Duration::from_secs(1);
        let ran_out = |last: Option<&Inbox>| never_asked("o1", &[PAY_HOLD], waited, last).category;

        assert_eq!(ran_out(None), FailureCategory::Relay);
        assert_eq!(
            ran_out(Some(&inbox(false, false))),
            FailureCategory::Product
        );
        assert_eq!(ran_out(Some(&inbox(true, false))), FailureCategory::Relay);
        assert_eq!(
            ran_out(Some(&inbox(false, true))),
            FailureCategory::EnvironmentMisconfiguration
        );
    }

    #[test]
    fn a_recovery_reads_back_to_the_runs_start_and_a_minute_more() {
        let window = MessageWindow::SinceRunStart { started: 1_000 };
        let reads = [1_000, 1_060, 1_061, 1_000 + 3 * 3600];

        let minutes = reads.map(|now| window.minutes_at(now));

        assert_eq!(minutes, [1, 2, 3, 181]);
        for now in reads {
            assert!(!window.is_capped_at(now), "{now}");
        }
        assert_eq!(MessageWindow::Run.minutes_at(u64::MAX), 1440);
        assert!(!MessageWindow::Run.is_capped_at(u64::MAX));
    }

    #[test]
    fn a_run_that_started_beyond_the_clis_reach_is_read_a_week_back_and_capped() {
        let last_whole_minute = 1_000 + 10_079 * 60;
        let window = MessageWindow::SinceRunStart { started: 1_000 };
        let reads = [last_whole_minute, last_whole_minute + 1];

        let reach = reads.map(|now| window.minutes_at(now));
        let capped = reads.map(|now| window.is_capped_at(now));

        assert_eq!(reach, [10_080, 10_080]);
        assert_eq!(capped, [false, true]);
        assert!(MessageWindow::SinceRunStart { started: 0 }.is_capped());
    }

    #[test]
    fn a_clock_behind_the_runs_start_reads_a_week_back_without_counting_as_capped() {
        let window = MessageWindow::SinceRunStart { started: 1_000 };

        assert_eq!(window.minutes_at(999), 10_080);
        assert!(!window.is_capped_at(999));
        assert!(!window.is_capped_at(0));
    }
}
