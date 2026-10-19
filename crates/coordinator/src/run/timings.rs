//! Where one run's wall time went (spec §12): its phases, and the UI requests
//! each actor's session served. Diagnostics only: nothing here decides a
//! verdict, and a run whose timings cannot be written stands as it is.

use std::collections::BTreeMap;
use std::future::Future;
use std::sync::{Mutex, PoisonError};

use mortsom_ui_driver::timed::RequestTimes;
use serde::Serialize;
use tokio::time::Instant;

use crate::artifacts::{RunArtifacts, files};

/// One phase of a run, and how long it took.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PhaseTiming {
    /// What ran: `preflight`, `bring-up`, `bootstrap/<step>`, `scenario`,
    /// `attribution`, `discovery`, `relay-quiet`, `cleanup`, `payments`, `teardown`.
    pub phase: String,
    /// The actor it ran for, when it ran for one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub actor: Option<String>,
    /// When it started, in milliseconds since the run started.
    pub start_ms: u64,
    /// How long it took, in milliseconds.
    pub elapsed_ms: u64,
}

/// The run's phase clock, started with the run.
pub struct RunTimings {
    origin: Instant,
    phases: Mutex<Vec<PhaseTiming>>,
}

impl Default for RunTimings {
    fn default() -> Self {
        Self {
            origin: Instant::now(),
            phases: Mutex::new(Vec::new()),
        }
    }
}

impl RunTimings {
    /// Runs `work` as `phase`, of `actor` when given, and records how long
    /// it took. What `work` answers is passed on untouched.
    pub async fn time<T>(
        &self,
        phase: &str,
        actor: Option<&str>,
        work: impl Future<Output = T>,
    ) -> T {
        let started = Instant::now();
        let output = work.await;
        self.record(phase, actor, started, Instant::now());
        output
    }

    /// Records `phase` as having run from `started` to `ended`.
    pub fn record(&self, phase: &str, actor: Option<&str>, started: Instant, ended: Instant) {
        let millis =
            |duration: std::time::Duration| u64::try_from(duration.as_millis()).unwrap_or(u64::MAX);
        self.phases
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(PhaseTiming {
                phase: phase.to_owned(),
                actor: actor.map(str::to_owned),
                start_ms: millis(started.saturating_duration_since(self.origin)),
                elapsed_ms: millis(ended.saturating_duration_since(started)),
            });
    }

    /// The phases recorded so far, in the order they finished.
    pub fn phases(&self) -> Vec<PhaseTiming> {
        self.phases
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }
}

/// Per actor, what each kind of UI request cost.
pub type UiTimes = BTreeMap<String, BTreeMap<String, RequestTimes>>;

/// The `timings.json` artifact.
#[derive(Debug, Serialize)]
pub struct TimingsReport {
    /// The run's phases, in the order they finished.
    pub phases: Vec<PhaseTiming>,
    /// Per actor, what each kind of UI request cost.
    pub ui: UiTimes,
}

impl RunTimings {
    /// Writes `timings.json` with the phases recorded so far and `ui`. A
    /// failed write is logged and the run stands as it is.
    pub fn persist(&self, artifacts: &RunArtifacts, ui: UiTimes) {
        let report = TimingsReport {
            phases: self.phases(),
            ui,
        };
        if let Err(error) = artifacts.write_json(files::TIMINGS, &report) {
            tracing::warn!(%error, "cannot persist the run's timings");
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn a_phase_is_placed_from_the_run_start_and_measured_to_its_end() {
        let timings = RunTimings::default();
        let started = timings.origin + Duration::from_millis(1500);

        timings.record(
            "bootstrap/launch",
            Some("alice"),
            started,
            started + Duration::from_millis(4200),
        );

        assert_eq!(
            timings.phases(),
            [PhaseTiming {
                phase: "bootstrap/launch".into(),
                actor: Some("alice".into()),
                start_ms: 1500,
                elapsed_ms: 4200,
            }]
        );
    }

    #[tokio::test]
    async fn a_timed_phase_passes_its_answer_on_and_is_recorded() {
        let timings = RunTimings::default();

        let answer = timings.time("scenario", None, async { 7 }).await;

        assert_eq!(answer, 7);
        let phases = timings.phases();
        assert_eq!(phases.len(), 1);
        assert_eq!(phases[0].phase, "scenario");
        assert_eq!(phases[0].actor, None);
    }

    #[test]
    fn a_phase_without_an_actor_leaves_the_field_out() {
        let phase = PhaseTiming {
            phase: "cleanup".into(),
            actor: None,
            start_ms: 0,
            elapsed_ms: 12,
        };

        let json = serde_json::to_value(&phase).unwrap();

        assert_eq!(
            json,
            serde_json::json!({"phase": "cleanup", "start_ms": 0, "elapsed_ms": 12})
        );
    }
}
