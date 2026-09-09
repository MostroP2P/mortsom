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
        let summary = summary.into();
        // The console follows the trade as it happens: what an actor did,
        // what a screen showed, what cleanup decided. Relay and wallet
        // observations stay in the artifacts, where they are many.
        match kind {
            TimelineKind::Step | TimelineKind::UiState | TimelineKind::Cleanup => {
                tracing::info!(actor = actor.unwrap_or("-"), "{summary}");
            }
            TimelineKind::Assertion => tracing::info!("{summary}"),
            TimelineKind::RelayEvent | TimelineKind::Wallet => tracing::debug!("{summary}"),
        }
        lock(&self.entries).push(TimelineEntry {
            at_ms,
            kind,
            actor: actor.map(str::to_owned),
            summary,
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
        let heading = title.replace(['\r', '\n'], " ");
        let mut out = format!(
            "# {heading}\n\n| time (unix ms) | kind | actor | event | artifact |\n|---|---|---|---|---|\n"
        );
        for e in self.entries() {
            out.push_str(&format!(
                "| {} | {:?} | {} | {} | {} |\n",
                e.at_ms,
                e.kind,
                cell(e.actor.as_deref()),
                cell(Some(&e.summary)),
                cell(e.artifact.as_deref())
            ));
        }
        out
    }
}

/// One Markdown table cell. Text comes from apps, relays and wallets, so a
/// pipe or a line break in it would otherwise break the table apart.
fn cell(value: Option<&str>) -> String {
    let Some(value) = value.filter(|v| !v.is_empty()) else {
        return "-".to_owned();
    };
    value
        .replace('\\', "\\\\")
        .replace('|', "\\|")
        .replace(['\r', '\n'], " ")
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
#[allow(clippy::unwrap_used, clippy::expect_used)]
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
    /// Summaries, actor names and artifact ids come from apps, relays and
    /// wallets; none of them may break the table apart.
    #[test]
    fn hostile_text_cannot_break_the_table() {
        let t = Timeline::default();
        t.push_at(
            10,
            TimelineKind::Step,
            Some("ali|ce"),
            "order o1\nsecond line | pipe \\ backslash",
            Some("art|ifact\nid".into()),
        );

        let md = t.render("Run | with\na break");

        let rows: Vec<&str> = md.lines().filter(|l| l.starts_with("| 10 ")).collect();
        assert_eq!(rows.len(), 1, "one entry is one row: {md}");
        assert_eq!(
            rows[0].matches(" | ").count(),
            4,
            "five columns, four separators: {}",
            rows[0]
        );
        assert_eq!(
            md.lines().next().unwrap(),
            "# Run | with a break",
            "the heading stays on one line"
        );
    }
}
