//! The single ordered timeline joining UI steps, relay events, wallet
//! observations, assertions and cleanup (spec Phase 4.5).

use std::sync::Mutex;

use serde::{Deserialize, Serialize};

/// What kind of entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum TimelineKind {
    /// A domain step performed by an actor.
    Step,
    /// A UI state observed on a device.
    UiState,
    /// A relay event.
    RelayEvent,
    /// A wallet observation.
    Wallet,
    /// An assertion verdict.
    Assertion,
    /// A cleanup action.
    Cleanup,
}

/// One entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TimelineEntry {
    /// Wall-clock time (unix milliseconds).
    pub at_ms: u64,
    /// Kind.
    pub kind: TimelineKind,
    /// Actor, when applicable.
    pub actor: Option<String>,
    /// Redacted summary.
    pub summary: String,
    /// Artifact reference (step label, event id, payment hash), if any.
    pub artifact: Option<String>,
}

/// Thread-safe ordered log.
#[derive(Debug, Default)]
pub struct Timeline {
    entries: Mutex<Vec<TimelineEntry>>,
}

impl Timeline {
    /// Appends an entry stamped now.
    pub fn push(
        &self,
        kind: TimelineKind,
        actor: Option<&str>,
        summary: impl Into<String>,
        artifact: Option<String>,
    ) {
        self.push_at(now_ms(), kind, actor, summary, artifact);
    }

    /// Appends an entry with an explicit timestamp (merging observer logs).
    pub fn push_at(
        &self,
        at_ms: u64,
        kind: TimelineKind,
        actor: Option<&str>,
        summary: impl Into<String>,
        artifact: Option<String>,
    ) {
        lock(&self.entries).push(TimelineEntry {
            at_ms,
            kind,
            actor: actor.map(str::to_owned),
            summary: summary.into(),
            artifact,
        });
    }

    /// Entries sorted by time (stable for equal timestamps).
    pub fn entries(&self) -> Vec<TimelineEntry> {
        let mut entries = lock(&self.entries).clone();
        entries.sort_by_key(|e| e.at_ms);
        entries
    }

    /// Markdown rendering.
    pub fn render(&self, title: &str) -> String {
        let mut out = format!(
            "# {title}\n\n| time (unix ms) | kind | actor | event | artifact |\n|---|---|---|---|---|\n"
        );
        for e in self.entries() {
            out.push_str(&format!(
                "| {} | {:?} | {} | {} | {} |\n",
                e.at_ms,
                e.kind,
                e.actor.as_deref().unwrap_or("-"),
                e.summary.replace('|', "\\|"),
                e.artifact.as_deref().unwrap_or("-")
            ));
        }
        out
    }
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|p| p.into_inner())
}

/// Wall-clock milliseconds; a clock before the epoch degrades to zero.
pub fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entries_are_sorted_and_rendered() {
        let t = Timeline::default();
        t.push_at(
            20,
            TimelineKind::RelayEvent,
            None,
            "kind 38383 pending",
            Some("ev1".into()),
        );
        t.push_at(10, TimelineKind::Step, Some("alice"), "create|order", None);
        let entries = t.entries();
        assert_eq!(entries[0].at_ms, 10);
        let md = t.render("Run");
        assert!(md.contains("| 10 | Step | alice | create\\|order | - |"));
        assert!(md.contains("| 20 | RelayEvent | - | kind 38383 pending | ev1 |"));
    }
}
