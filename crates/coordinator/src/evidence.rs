//! Evidence checkpoints: screenshot plus accessibility hierarchy per actor
//! step, numbered in order (spec §4.3, §11). A command-line session has no
//! screen and keeps its transcript (`cli.jsonl`) instead.

use std::sync::atomic::{AtomicU32, Ordering};

use mortsom_ui_driver::ui::UiSession;
use tracing::warn;

use crate::artifacts::RunArtifacts;

/// Numbers steps per actor and writes their evidence.
#[derive(Debug, Default)]
pub struct StepRecorder {
    counters: [AtomicU32; 2],
}

impl StepRecorder {
    /// Records the current screen of `actor` under `name`, or for a
    /// command-line session the transcript of the step's invocations.
    /// Failures are logged, never fatal: evidence must not break the
    /// scenario.
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
        if ui.command_line().is_some() {
            record_transcript(artifacts, actor, &step, ui).await;
            return step;
        }
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

/// A command-line client has no screen: its evidence for a step is the
/// sanitized transcript of the invocations the step made.
async fn record_transcript(artifacts: &RunArtifacts, actor: &str, step: &str, ui: &dyn UiSession) {
    match ui.page_source().await {
        Ok(lines) if lines.is_empty() => {}
        Ok(lines) => {
            if let Err(e) = artifacts.write_step_text(actor, step, "cli.jsonl", &lines) {
                warn!(actor, step, error = %e, "cannot write CLI transcript");
            }
        }
        Err(e) => warn!(actor, step, error = %e, "cannot read CLI transcript"),
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

    fn files_under(dir: &std::path::Path) -> Vec<String> {
        let mut names = Vec::new();
        let mut pending = vec![dir.to_path_buf()];
        while let Some(path) = pending.pop() {
            for entry in std::fs::read_dir(&path).into_iter().flatten().flatten() {
                if entry.path().is_dir() {
                    pending.push(entry.path());
                } else {
                    names.push(entry.file_name().to_string_lossy().into_owned());
                }
            }
        }
        names
    }

    #[tokio::test]
    #[allow(clippy::unwrap_used)]
    async fn a_command_line_step_keeps_its_transcript_instead_of_a_screen() {
        let dir = tempfile::tempdir().unwrap();
        let artifacts = RunArtifacts::create(
            dir.path(),
            mortsom_common::RunId::new(),
            mortsom_common::Redactor::new(),
        )
        .unwrap();
        let ui = mortsom_ui_driver::ui::tests_support::ScriptedUi::console();
        ui.present(&["order.take-sell"]);
        let idle = mortsom_ui_driver::ui::tests_support::ScriptedUi::console();
        let recorder = StepRecorder::default();

        let step = recorder
            .checkpoint(&artifacts, "alice", "take-order", &ui)
            .await;
        let quiet = recorder
            .checkpoint(&artifacts, "alice", "nothing-ran", &idle)
            .await;

        assert_eq!(step, "01-take-order");
        assert_eq!(quiet, "02-nothing-ran");
        let files = files_under(artifacts.dir());
        assert_eq!(
            files.iter().filter(|f| f.ends_with("cli.jsonl")).count(),
            1,
            "{files:?}"
        );
        assert!(
            !files.iter().any(|f| f.ends_with("screen.png")),
            "{files:?}"
        );
    }
}
