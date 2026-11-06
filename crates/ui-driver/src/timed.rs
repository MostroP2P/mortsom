//! A UI session that times every request it forwards: how many of each kind
//! an actor's session served, how long they took in total and at worst, how
//! many failed, and how many its caller gave up on before they ended.
//! Diagnostics only (spec §12): it changes neither what a request does nor
//! what it answers.

use std::collections::BTreeMap;
use std::future::Future;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use async_trait::async_trait;
use serde::Serialize;
use tokio::time::Instant;

use crate::error::UiError;
use crate::ui::UiSession;

/// What one kind of request cost over a session.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct RequestTimes {
    /// Requests served.
    pub count: u64,
    /// Requests that failed.
    pub errors: u64,
    /// Requests their caller gave up on before they ended: a deadline that
    /// expired around them.
    pub cancelled: u64,
    /// Time spent in them, in milliseconds.
    pub total_ms: u64,
    /// The longest one, in milliseconds.
    pub max_ms: u64,
}

/// How one request ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Ended {
    Answered,
    Failed,
    /// Dropped before it ended.
    Cancelled,
}

impl RequestTimes {
    fn add(&mut self, elapsed: Duration, ended: Ended) {
        let ms = u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX);
        self.count += 1;
        self.errors += u64::from(ended == Ended::Failed);
        self.cancelled += u64::from(ended == Ended::Cancelled);
        self.total_ms = self.total_ms.saturating_add(ms);
        self.max_ms = self.max_ms.max(ms);
    }
}

/// One request on its way, recorded when it is dropped: once it ended, or
/// unfinished, when its caller gives up on it.
struct InFlight<'a> {
    times: &'a Mutex<BTreeMap<&'static str, RequestTimes>>,
    kind: &'static str,
    started: Instant,
    ended: Ended,
}

impl Drop for InFlight<'_> {
    fn drop(&mut self) {
        self.times
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .entry(self.kind)
            .or_default()
            .add(self.started.elapsed(), self.ended);
    }
}

/// Forwards every request to the session it wraps, and times it.
pub struct TimedUi {
    inner: Arc<dyn UiSession>,
    times: Mutex<BTreeMap<&'static str, RequestTimes>>,
}

impl TimedUi {
    /// Times the requests `inner` serves from now on.
    pub fn new(inner: Arc<dyn UiSession>) -> Self {
        Self {
            inner,
            times: Mutex::new(BTreeMap::new()),
        }
    }

    /// What each kind of request cost so far, by the trait method's name.
    pub fn times(&self) -> BTreeMap<String, RequestTimes> {
        self.times
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .iter()
            .map(|(kind, times)| ((*kind).to_owned(), *times))
            .collect()
    }

    async fn timed<T>(
        &self,
        kind: &'static str,
        request: impl Future<Output = Result<T, UiError>>,
    ) -> Result<T, UiError> {
        let mut flight = InFlight {
            times: &self.times,
            kind,
            started: Instant::now(),
            ended: Ended::Cancelled,
        };
        let result = request.await;
        flight.ended = if result.is_ok() {
            Ended::Answered
        } else {
            Ended::Failed
        };
        result
    }
}

#[async_trait]
impl UiSession for TimedUi {
    async fn exists(&self, id: &str) -> Result<bool, UiError> {
        self.timed("exists", self.inner.exists(id)).await
    }
    async fn exists_anywhere(&self, id: &str) -> Result<bool, UiError> {
        self.timed("exists_anywhere", self.inner.exists_anywhere(id))
            .await
    }
    async fn wait_for(&self, id: &str, timeout: Duration) -> Result<(), UiError> {
        self.timed("wait_for", self.inner.wait_for(id, timeout))
            .await
    }
    async fn wait_gone(&self, id: &str, timeout: Duration) -> Result<(), UiError> {
        self.timed("wait_gone", self.inner.wait_gone(id, timeout))
            .await
    }
    async fn tap(&self, id: &str) -> Result<(), UiError> {
        self.timed("tap", self.inner.tap(id)).await
    }
    async fn type_text(&self, id: &str, text: &str) -> Result<(), UiError> {
        self.timed("type_text", self.inner.type_text(id, text))
            .await
    }
    async fn text_of(&self, id: &str) -> Result<String, UiError> {
        self.timed("text_of", self.inner.text_of(id)).await
    }
    async fn is_enabled(&self, id: &str) -> Result<bool, UiError> {
        self.timed("is_enabled", self.inner.is_enabled(id)).await
    }
    async fn label_of(&self, id: &str) -> Result<String, UiError> {
        self.timed("label_of", self.inner.label_of(id)).await
    }
    async fn screenshot_png(&self) -> Result<Vec<u8>, UiError> {
        self.timed("screenshot_png", self.inner.screenshot_png())
            .await
    }
    async fn page_source(&self) -> Result<String, UiError> {
        self.timed("page_source", self.inner.page_source()).await
    }
    async fn background(&self, secs: u64) -> Result<(), UiError> {
        self.timed("background", self.inner.background(secs)).await
    }
    async fn terminate_app(&self) -> Result<(), UiError> {
        self.timed("terminate_app", self.inner.terminate_app())
            .await
    }
    async fn activate_app(&self) -> Result<(), UiError> {
        self.timed("activate_app", self.inner.activate_app()).await
    }
    async fn clear_app(&self) -> Result<(), UiError> {
        self.timed("clear_app", self.inner.clear_app()).await
    }
    async fn set_connectivity(&self, enabled: bool) -> Result<(), UiError> {
        self.timed("set_connectivity", self.inner.set_connectivity(enabled))
            .await
    }
    async fn set_orientation(&self, landscape: bool) -> Result<(), UiError> {
        self.timed("set_orientation", self.inner.set_orientation(landscape))
            .await
    }
    async fn hide_keyboard(&self) -> Result<(), UiError> {
        self.timed("hide_keyboard", self.inner.hide_keyboard())
            .await
    }
    async fn diagnostics(&self) -> Vec<(String, String)> {
        self.inner.diagnostics().await
    }
    fn command_line(&self) -> Option<&dyn std::any::Any> {
        self.inner.command_line()
    }
    async fn end(&self) -> Result<(), UiError> {
        self.timed("end", self.inner.end()).await
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::ui::tests_support::ScriptedUi;

    fn timed(present: &[&str]) -> (Arc<ScriptedUi>, TimedUi) {
        let scripted = Arc::new(ScriptedUi::default());
        scripted.present(present);
        (scripted.clone(), TimedUi::new(scripted))
    }

    #[tokio::test]
    async fn every_request_reaches_the_session_and_is_counted_by_kind() {
        let (scripted, ui) = timed(&["order.status"]);

        assert!(ui.exists("order.status").await.unwrap());
        assert!(!ui.exists("order.id").await.unwrap());
        ui.tap("order.status").await.unwrap();

        let times = ui.times();
        assert_eq!(times["exists"].count, 2);
        assert_eq!(times["tap"].count, 1);
        assert_eq!(times["tap"].errors, 0);
        assert!(
            scripted.calls().contains(&"tap order.status".to_owned()),
            "{:?}",
            scripted.calls()
        );
    }

    #[test]
    fn the_command_line_behind_a_timed_session_is_the_inner_sessions() {
        let screen = TimedUi::new(Arc::new(ScriptedUi::default()));
        let console = TimedUi::new(Arc::new(ScriptedUi::console()));

        assert!(screen.command_line().is_none());
        let inner = console
            .command_line()
            .and_then(|any| any.downcast_ref::<ScriptedUi>());
        assert!(inner.is_some());
    }

    #[test]
    fn an_unsupported_control_names_itself() {
        let error = UiError::Unsupported("screenshots");
        assert_eq!(
            error.to_string(),
            "screenshots is not supported by this client"
        );
    }

    #[tokio::test]
    async fn a_failed_request_is_counted_as_an_error_and_answered_as_it_failed() {
        let (_, ui) = timed(&[]);

        let failed = ui.wait_for("trade.release", Duration::ZERO).await;

        assert!(matches!(failed, Err(UiError::WaitTimeout { .. })));
        let times = ui.times()["wait_for"];
        assert_eq!((times.count, times.errors), (1, 1));
    }

    /// A deadline that expires around a request drops it unfinished; that
    /// request is the one most likely to explain the deadline.
    #[tokio::test]
    async fn a_request_its_caller_gives_up_on_is_counted_with_its_time() {
        let (scripted, ui) = timed(&[]);
        scripted.slow_screenshots(Duration::from_secs(60));

        let gave_up = tokio::time::timeout(Duration::from_millis(20), ui.screenshot_png()).await;

        assert!(gave_up.is_err());
        let times = ui.times()["screenshot_png"];
        assert_eq!((times.count, times.errors, times.cancelled), (1, 0, 1));
        assert!(times.total_ms >= 20, "{times:?}");
    }

    #[test]
    fn the_total_adds_up_and_the_longest_request_is_kept() {
        let mut times = RequestTimes::default();

        times.add(Duration::from_millis(30), Ended::Answered);
        times.add(Duration::from_millis(10), Ended::Failed);
        times.add(Duration::from_millis(5), Ended::Cancelled);

        assert_eq!(
            times,
            RequestTimes {
                count: 3,
                errors: 1,
                cancelled: 1,
                total_ms: 45,
                max_ms: 30,
            }
        );
    }

    #[tokio::test]
    async fn diagnostics_and_end_pass_through() {
        let (scripted, ui) = timed(&[]);

        assert!(ui.diagnostics().await.is_empty());
        ui.end().await.unwrap();

        assert!(scripted.calls().contains(&"end session".to_owned()));
        assert_eq!(ui.times()["end"].count, 1);
    }
}
