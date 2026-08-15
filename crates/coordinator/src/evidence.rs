//! Evidence checkpoints: screenshot plus accessibility hierarchy per actor
//! step, numbered in order (spec §4.3, §11).

use std::sync::atomic::{AtomicU32, Ordering};

use mortsom_android_driver::ui::UiSession;
use tracing::warn;

use crate::artifacts::RunArtifacts;

/// Numbers steps per actor and writes their evidence.
#[derive(Debug, Default)]
pub struct StepRecorder {
    counters: [AtomicU32; 2],
}

impl StepRecorder {
    /// Records the current screen of `actor` under `name`. Failures are
    /// logged, never fatal: evidence must not break the scenario.
    pub async fn checkpoint(
        &self,
        artifacts: &RunArtifacts,
        actor: &str,
        name: &str,
        ui: &dyn UiSession,
    ) -> String {
        let index = actor_index(actor);
        let seq = self.counters[index].fetch_add(1, Ordering::Relaxed) + 1;
        let step = format!("{seq:02}-{}", sanitize(name));
        match ui.screenshot_png().await {
            Ok(png) => {
                if let Err(e) = artifacts.write_step_bytes(actor, &step, "screen.png", &png) {
                    warn!(actor, step, error = %e, "cannot write screenshot");
                }
            }
            Err(e) => warn!(actor, step, error = %e, "cannot capture screenshot"),
        }
        match ui.page_source().await {
            Ok(xml) => {
                if let Err(e) = artifacts.write_step_text(actor, &step, "ui.xml", &xml) {
                    warn!(actor, step, error = %e, "cannot write hierarchy");
                }
            }
            Err(e) => warn!(actor, step, error = %e, "cannot capture hierarchy"),
        }
        step
    }
}

fn actor_index(actor: &str) -> usize {
    usize::from(actor == "bob")
}

/// Keeps step names filesystem-safe.
pub fn sanitize(name: &str) -> String {
    name.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' {
                c
            } else {
                '-'
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_replaces_unsafe_characters() {
        assert_eq!(sanitize("configure nwc/alice"), "configure-nwc-alice");
    }

    #[test]
    fn actors_map_to_slots() {
        assert_eq!(actor_index("alice"), 0);
        assert_eq!(actor_index("bob"), 1);
    }
}
