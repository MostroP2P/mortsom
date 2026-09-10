//! Bounded waits: every wait is a predicate polled until a deadline, never
//! a fixed sleep (spec §7).

use std::future::Future;
use std::time::Duration;

/// Polls `probe` every `interval` until it returns `Some`, or `timeout`
/// elapses. The last probe runs at the deadline so a late success is seen.
pub async fn wait_until<T, F, Fut>(timeout: Duration, interval: Duration, mut probe: F) -> Option<T>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Option<T>>,
{
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        match probe_before(deadline, probe()).await {
            Probe::Found(value) => return Some(value),
            Probe::Deadline => return None,
            Probe::Absent => {}
        }
        let now = tokio::time::Instant::now();
        if now >= deadline {
            return None;
        }
        tokio::time::sleep(interval.min(deadline - now)).await;
    }
}

/// Outcome of one probe attempt bounded by the deadline.
enum Probe<T> {
    Found(T),
    Absent,
    Deadline,
}

/// Races one probe against the deadline: a probe that never answers must
/// not push the caller past its timeout, and its late answer is discarded.
async fn probe_before<T, Fut>(deadline: tokio::time::Instant, probe: Fut) -> Probe<T>
where
    Fut: Future<Output = Option<T>>,
{
    tokio::select! {
        biased;
        answer = probe => match answer {
            Some(value) => Probe::Found(value),
            None => Probe::Absent,
        },
        () = tokio::time::sleep_until(deadline) => Probe::Deadline,
    }
}

/// Negative observation: `probe` must stay `None` for the whole `window`.
/// Returns the offending value if it appeared.
pub async fn stays_absent<T, F, Fut>(
    window: Duration,
    interval: Duration,
    mut probe: F,
) -> Option<T>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Option<T>>,
{
    let deadline = tokio::time::Instant::now() + window;
    loop {
        match probe_before(deadline, probe()).await {
            Probe::Found(value) => return Some(value),
            // A probe still running when the window closes proves nothing
            // appeared within it, which is what this function asserts.
            Probe::Deadline => return None,
            Probe::Absent => {}
        }
        let now = tokio::time::Instant::now();
        if now >= deadline {
            return None;
        }
        tokio::time::sleep(interval.min(deadline - now)).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    #[tokio::test]
    async fn wait_until_returns_on_success_and_none_on_timeout() {
        let calls = AtomicU32::new(0);
        let found = wait_until(Duration::from_secs(1), Duration::from_millis(1), || async {
            let n = calls.fetch_add(1, Ordering::Relaxed);
            (n >= 2).then_some(n)
        })
        .await;
        assert_eq!(found, Some(2));

        let none: Option<u32> = wait_until(
            Duration::from_millis(5),
            Duration::from_millis(1),
            || async { None },
        )
        .await;
        assert!(none.is_none());
    }

    #[tokio::test]
    async fn stays_absent_reports_intrusions() {
        let seen: Option<u32> = stays_absent(
            Duration::from_millis(5),
            Duration::from_millis(1),
            || async { None },
        )
        .await;
        assert!(seen.is_none());
        let seen = stays_absent(
            Duration::from_millis(50),
            Duration::from_millis(1),
            || async { Some(7u32) },
        )
        .await;
        assert_eq!(seen, Some(7));
    }
    /// A probe that never answers (a wedged WebDriver call) must not push the
    /// caller past its own deadline.
    #[tokio::test]
    async fn a_probe_that_never_answers_still_ends_at_the_deadline() {
        let started = tokio::time::Instant::now();

        let found: Option<()> = wait_until(
            Duration::from_millis(50),
            Duration::from_millis(5),
            || async {
                std::future::pending::<()>().await;
                unreachable!()
            },
        )
        .await;
        let absent: Option<()> = stays_absent(
            Duration::from_millis(50),
            Duration::from_millis(5),
            || async {
                std::future::pending::<()>().await;
                unreachable!()
            },
        )
        .await;

        assert!(found.is_none());
        assert!(absent.is_none());
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "both waits returned at their deadlines"
        );
    }
}
