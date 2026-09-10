//! Evidence boundary the kit writes through; the coordinator implements it
//! over the run artifacts.

use async_trait::async_trait;
use mortsom_common::Secret;
use mortsom_ui_driver::ui::UiSession;

/// Where scenario evidence goes.
#[async_trait]
pub trait EvidenceSink: Send + Sync {
    /// Records a screenshot and hierarchy for `actor` under `name`; returns
    /// the step label used in artifact names.
    async fn checkpoint(&self, actor: &str, name: &str, ui: &dyn UiSession) -> String;
    /// Registers a secret learned mid-run so later artifacts redact it.
    fn protect(&self, secret: &Secret);
}

/// Sink that records nothing (unit tests).
#[derive(Debug, Default)]
pub struct NullSink;

#[async_trait]
impl EvidenceSink for NullSink {
    async fn checkpoint(&self, actor: &str, name: &str, _ui: &dyn UiSession) -> String {
        format!("{actor}:{name}")
    }
    fn protect(&self, _secret: &Secret) {}
}
