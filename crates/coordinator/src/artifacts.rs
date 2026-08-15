//! Artifact directory layout for one run (spec §11). Creating the layout
//! never writes a secret; text written through [`RunArtifacts::write_text`]
//! passes through the run redactor.

use std::io;
use std::path::{Path, PathBuf};

use mortsom_common::{Redactor, RunId};
use serde::Serialize;

/// File names inside `artifacts/<run>/`.
pub mod files {
    /// Run summary.
    pub const SUMMARY: &str = "summary.json";
    /// Ordered human-readable timeline.
    pub const TIMELINE: &str = "timeline.md";
    /// Environment description (lane, app profile, versions).
    pub const ENVIRONMENT: &str = "environment.json";
    /// Notification observations.
    pub const NOTIFICATIONS: &str = "notifications.jsonl";
    /// Relay event observations.
    pub const RELAY_EVENTS: &str = "relay-events.jsonl";
    /// Wallet payment observations.
    pub const WALLET_PAYMENTS: &str = "wallet-payments.json";
    /// Device logcat.
    pub const DEVICE_LOGCAT: &str = "device-logcat.txt";
    /// Cleanup result.
    pub const CLEANUP: &str = "cleanup.json";
    /// Redacted run log.
    pub const RUN_LOG: &str = mortsom_common::logging::RUN_LOG_FILE;
}

/// Actor names with their own step directories.
pub const ACTORS: &[&str] = &["alice", "bob"];

/// Handle to one run's artifact directory.
#[derive(Debug, Clone)]
pub struct RunArtifacts {
    run_id: RunId,
    dir: PathBuf,
    redactor: Redactor,
}

impl RunArtifacts {
    /// Creates `root/<run_id>` with the actor step directories.
    pub fn create(root: &Path, run_id: RunId, redactor: Redactor) -> io::Result<Self> {
        let dir = root.join(run_id.as_str());
        std::fs::create_dir_all(&dir)?;
        for actor in ACTORS {
            std::fs::create_dir_all(dir.join(actor).join("steps"))?;
        }
        Ok(Self {
            run_id,
            dir,
            redactor,
        })
    }

    /// The run identifier.
    pub fn run_id(&self) -> &RunId {
        &self.run_id
    }

    /// The run directory.
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Path of a top-level artifact file. `name` must be a single safe segment.
    pub fn file(&self, name: &str) -> io::Result<PathBuf> {
        Ok(self.dir.join(safe_segment(name)?))
    }

    /// Path of an actor step artifact, e.g. `alice/steps/03-create-order-screen.png`.
    pub fn step_file(&self, actor: &str, step: &str, suffix: &str) -> io::Result<PathBuf> {
        let name = format!("{}-{}", safe_segment(step)?, safe_segment(suffix)?);
        Ok(self.dir.join(safe_segment(actor)?).join("steps").join(name))
    }

    /// Serializes `value` as pretty JSON after redacting every string value.
    /// Redaction happens on the value tree, before escaping, so a secret
    /// containing quotes or backslashes is still caught.
    pub fn write_json<T: Serialize>(&self, name: &str, value: &T) -> io::Result<()> {
        let json = serde_json::to_string_pretty(&self.redacted_value(value)?)?;
        std::fs::write(self.file(name)?, json)
    }

    /// Writes free text produced by external tools after redaction.
    pub fn write_text(&self, name: &str, text: &str) -> io::Result<()> {
        std::fs::write(self.file(name)?, self.redactor.redact(text))
    }

    /// Appends one JSON line, redacted on the value tree before escaping.
    pub fn append_jsonl<T: Serialize>(&self, name: &str, value: &T) -> io::Result<()> {
        use std::io::Write;
        let line = serde_json::to_string(&self.redacted_value(value)?)?;
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.file(name)?)?;
        writeln!(file, "{line}")
    }

    fn redacted_value<T: Serialize>(&self, value: &T) -> io::Result<serde_json::Value> {
        let tree = serde_json::to_value(value)?;
        Ok(redact_tree(tree, &self.redactor))
    }
}

/// Redacts every string leaf of a JSON tree.
pub fn redact_tree(value: serde_json::Value, redactor: &Redactor) -> serde_json::Value {
    use serde_json::Value;
    match value {
        Value::String(text) => Value::String(redactor.redact(&text)),
        Value::Array(items) => Value::Array(
            items
                .into_iter()
                .map(|v| redact_tree(v, redactor))
                .collect(),
        ),
        Value::Object(map) => Value::Object(
            map.into_iter()
                .map(|(k, v)| (k, redact_tree(v, redactor)))
                .collect(),
        ),
        other => other,
    }
}

/// Accepts only a single path segment made of `[A-Za-z0-9._-]` that is not
/// `.` or `..`, so artifact names can never escape the run directory.
pub fn safe_segment(name: &str) -> io::Result<&str> {
    let ok = !name.is_empty()
        && name != "."
        && name != ".."
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'));
    if ok {
        Ok(name)
    } else {
        Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("unsafe artifact name `{name}`"),
        ))
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use mortsom_common::Secret;

    fn artifacts(dir: &Path) -> RunArtifacts {
        let redactor = Redactor::new().with_secret(&Secret::new("hunter2"));
        RunArtifacts::create(dir, "20260815T000000Z-abc123".parse().unwrap(), redactor).unwrap()
    }

    #[test]
    fn create_builds_the_spec_layout() {
        let root = tempfile::tempdir().unwrap();

        let artifacts = artifacts(root.path());

        assert_eq!(artifacts.dir(), root.path().join("20260815T000000Z-abc123"));
        assert!(artifacts.dir().join("alice/steps").is_dir());
        assert!(artifacts.dir().join("bob/steps").is_dir());
        assert_eq!(
            artifacts
                .step_file("alice", "03-create-order", "screen.png")
                .unwrap(),
            artifacts
                .dir()
                .join("alice/steps/03-create-order-screen.png")
        );
        assert_eq!(artifacts.run_id().as_str(), "20260815T000000Z-abc123");
        assert!(artifacts.step_file("alice", "../x", "png").is_err());
        assert!(artifacts.file("a/b").is_err());
        assert!(artifacts.file("..").is_err());
    }

    #[test]
    fn text_and_jsonl_are_redacted_but_json_is_typed() {
        let root = tempfile::tempdir().unwrap();
        let artifacts = artifacts(root.path());

        artifacts
            .write_text(files::DEVICE_LOGCAT, "token hunter2 seen\n")
            .unwrap();
        artifacts
            .append_jsonl(files::RELAY_EVENTS, &serde_json::json!({"note": "hunter2"}))
            .unwrap();
        artifacts
            .append_jsonl(files::RELAY_EVENTS, &serde_json::json!({"seq": 2}))
            .unwrap();
        artifacts
            .write_json(
                files::SUMMARY,
                &serde_json::json!({"secret": Secret::new("hunter2")}),
            )
            .unwrap();

        let logcat =
            std::fs::read_to_string(artifacts.file(files::DEVICE_LOGCAT).unwrap()).unwrap();
        let events = std::fs::read_to_string(artifacts.file(files::RELAY_EVENTS).unwrap()).unwrap();
        let summary = std::fs::read_to_string(artifacts.file(files::SUMMARY).unwrap()).unwrap();
        assert_eq!(logcat, "token <redacted> seen\n");
        assert_eq!(events.lines().count(), 2);
        assert!(!events.contains("hunter2"));
        assert!(summary.contains("<redacted>"));
    }

    #[test]
    fn secrets_with_json_special_characters_are_redacted_before_escaping() {
        let root = tempfile::tempdir().unwrap();
        let redactor = Redactor::new().with_secret(&Secret::new("qu\"ote\\slash"));
        let artifacts = RunArtifacts::create(
            root.path(),
            "20260815T000000Z-abc124".parse().unwrap(),
            redactor,
        )
        .unwrap();

        artifacts
            .append_jsonl(
                files::NOTIFICATIONS,
                &serde_json::json!({"body": "x qu\"ote\\slash y"}),
            )
            .unwrap();

        let text = std::fs::read_to_string(artifacts.file(files::NOTIFICATIONS).unwrap()).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(text.trim()).unwrap();
        assert_eq!(parsed["body"], "x <redacted> y");
    }
}
