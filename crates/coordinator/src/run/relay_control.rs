//! Relay fault injection for scenarios: restarts the run relay container in
//! place and replays already-signed events onto it (spec §9.2). Scoped to
//! this run's relay only.

use std::sync::Arc;
use std::time::Duration;

use mortsom_environment::docker::ContainerRuntime;
use mortsom_environment::relay::replay::replay_events;
use mortsom_environment::relay::{ReadinessProbe, RelayHandle, restart_relay};
use mortsom_scenario_kit::recovery::RelayControl;

/// Deadline for a replay publish.
const REPLAY_TIMEOUT: Duration = Duration::from_secs(15);

/// Control over the run relay container.
pub struct LiveRelayControl {
    docker: Arc<dyn ContainerRuntime>,
    probe: Arc<dyn ReadinessProbe>,
    handle: RelayHandle,
    ready_timeout: Duration,
}

impl LiveRelayControl {
    /// Binds to the run relay `handle`.
    pub fn new(
        docker: Arc<dyn ContainerRuntime>,
        probe: Arc<dyn ReadinessProbe>,
        handle: RelayHandle,
        ready_timeout: Duration,
    ) -> Self {
        Self {
            docker,
            probe,
            handle,
            ready_timeout,
        }
    }
}

#[async_trait::async_trait]
impl RelayControl for LiveRelayControl {
    async fn restart(&self) -> Result<(), String> {
        restart_relay(
            self.docker.as_ref(),
            self.probe.as_ref(),
            &self.handle,
            self.ready_timeout,
        )
        .await
        .map_err(|e| e.to_string())
    }

    async fn replay(&self, events_json: &[String]) -> Result<usize, String> {
        replay_events(&self.handle.host_url(), events_json, REPLAY_TIMEOUT)
            .await
            .map_err(|e| e.to_string())
    }
}
