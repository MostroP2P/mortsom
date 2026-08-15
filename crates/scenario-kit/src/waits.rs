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
        if let Some(value) = probe().await {
            return Some(value);
        }
        let now = tokio::time::Instant::now();
        if now >= deadline {
            return None;
        }
        tokio::time::sleep(interval.min(deadline - now)).await;
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
        if let Some(value) = probe().await {
            return Some(value);
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
}
