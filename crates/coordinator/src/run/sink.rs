//! Evidence sink over the run artifacts.

use std::sync::Arc;

use async_trait::async_trait;
use mortsom_android_driver::ui::UiSession;
use mortsom_common::Secret;
use mortsom_scenario_kit::evidence::EvidenceSink;

use crate::artifacts::RunArtifacts;
use crate::evidence::StepRecorder;

/// Writes checkpoints into the run artifacts and learns secrets mid-run.
pub struct ArtifactSink {
    artifacts: RunArtifacts,
    recorder: Arc<StepRecorder>,
}

impl ArtifactSink {
    /// Creates the sink.
    pub fn new(artifacts: RunArtifacts, recorder: Arc<StepRecorder>) -> Self {
        Self {
            artifacts,
            recorder,
        }
    }
}

#[async_trait]
impl EvidenceSink for ArtifactSink {
    async fn checkpoint(&self, actor: &str, name: &str, ui: &dyn UiSession) -> String {
        self.recorder
            .checkpoint(&self.artifacts, actor, name, ui)
            .await
    }
    fn protect(&self, secret: &Secret) {
        self.artifacts.add_secret(secret);
    }
}
