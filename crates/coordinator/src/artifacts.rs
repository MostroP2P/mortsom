//! Artifact directory layout for one run (spec §11). Creating the layout
//! never writes a secret; text written through [`RunArtifacts::write_text`]
//! passes through the run redactor.

use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

use mortsom_common::{Redactor, RunId, Secret};
use serde::Serialize;

mod platform;
pub(crate) use platform::recorded_platform;

/// File names inside `artifacts/<run>/`.
pub mod files {
    /// Run summary.
    pub const SUMMARY: &str = "summary.json";
    /// Ordered human-readable timeline.
    pub const TIMELINE: &str = "timeline.md";
    /// Structured timeline (same entries as `timeline.md`), for tooling.
    pub const TIMELINE_JSON: &str = "timeline.json";
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

/// Handle to one run's artifact directory. Clones share the redactor, so a
/// secret learned mid-run (an invoice the app displayed) is scrubbed from
/// every later artifact.
#[derive(Debug, Clone)]
pub struct RunArtifacts {
    run_id: RunId,
    dir: PathBuf,
    redactor: Arc<RwLock<Redactor>>,
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
            redactor: Arc::new(RwLock::new(redactor)),
        })
    }

    /// Registers a secret learned during the run.
    pub fn add_secret(&self, secret: &Secret) {
        self.redactor
            .write()
            .unwrap_or_else(|p| p.into_inner())
            .add_secret(secret);
    }

    /// Redacts free text with the current secret set.
    pub fn redact(&self, text: &str) -> String {
        self.redactor
            .read()
            .unwrap_or_else(|p| p.into_inner())
            .redact(text)
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

    /// Retains verified public stack hashes only in structured run metadata.
    /// Known secrets still win; every other field uses the normal scrubber.
    pub(crate) fn write_metadata_json<T: Serialize>(
        &self,
        name: &str,
        value: &T,
        stack: Option<&crate::stack::StackSnapshot>,
        app_artifact: Option<&crate::fingerprint::BundleFingerprint>,
    ) -> io::Result<()> {
        let Some(stack) = stack else {
            return self.write_json(name, value);
        };
        if !matches!(name, files::SUMMARY | files::ENVIRONMENT) {
            return Err(io::Error::other(
                "public metadata is restricted to run reports",
            ));
        }
        stack.daemon_image.validate().map_err(io::Error::other)?;
        nostr_sdk::prelude::PublicKey::parse(&stack.daemon_pubkey).map_err(io::Error::other)?;
        let mut tree = self.redacted_value(value)?;
        let redactor = self.redactor.read().unwrap_or_else(|p| p.into_inner());
        restore_stack(&mut tree, stack, &redactor)?;
        restore_bundle(&mut tree, app_artifact, &redactor)?;
        std::fs::write(self.file(name)?, serde_json::to_string_pretty(&tree)?)
    }

    /// Writes free text produced by external tools after redaction.
    pub fn write_text(&self, name: &str, text: &str) -> io::Result<()> {
        std::fs::write(self.file(name)?, self.redact(text))
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

    /// Writes a binary step artifact (screenshot). Pixels cannot be text-
    /// redacted; screenshots are published only through the CI redaction
    /// step described in spec §11.
    pub fn write_step_bytes(
        &self,
        actor: &str,
        step: &str,
        suffix: &str,
        bytes: &[u8],
    ) -> io::Result<()> {
        std::fs::write(self.step_file(actor, step, suffix)?, bytes)
    }

    /// Writes a redacted text step artifact (accessibility hierarchy).
    pub fn write_step_text(
        &self,
        actor: &str,
        step: &str,
        suffix: &str,
        text: &str,
    ) -> io::Result<()> {
        std::fs::write(self.step_file(actor, step, suffix)?, self.redact(text))
    }

    fn redacted_value<T: Serialize>(&self, value: &T) -> io::Result<serde_json::Value> {
        let tree = serde_json::to_value(value)?;
        let redactor = self.redactor.read().unwrap_or_else(|p| p.into_inner());
        Ok(redact_tree(tree, &redactor))
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
    fn step_artifacts_are_written_per_actor() {
        let root = tempfile::tempdir().unwrap();
        let artifacts = artifacts(root.path());

        artifacts
            .write_step_bytes("alice", "01-launch", "screen.png", b"\x89PNG")
            .unwrap();
        artifacts
            .write_step_text("bob", "01-launch", "ui.xml", "<a>hunter2</a>")
            .unwrap();

        assert!(
            artifacts
                .dir()
                .join("alice/steps/01-launch-screen.png")
                .is_file()
        );
        let xml =
            std::fs::read_to_string(artifacts.dir().join("bob/steps/01-launch-ui.xml")).unwrap();
        assert_eq!(xml, "<a><redacted></a>");
        assert!(
            artifacts
                .write_step_bytes("eve", "../x", "png", b"")
                .is_err()
        );
    }

    #[test]
    fn secrets_learned_mid_run_are_redacted_by_every_clone() {
        let root = tempfile::tempdir().unwrap();
        let artifacts = artifacts(root.path());
        let clone = artifacts.clone();

        artifacts.write_text("before.txt", "lnbc1invoice").unwrap();
        clone.add_secret(&Secret::new("lnbc1invoice"));
        artifacts.write_text("after.txt", "lnbc1invoice").unwrap();

        assert_eq!(
            std::fs::read_to_string(artifacts.file("before.txt").unwrap()).unwrap(),
            "lnbc1invoice"
        );
        assert_eq!(
            std::fs::read_to_string(artifacts.file("after.txt").unwrap()).unwrap(),
            "<redacted>"
        );
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

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod metadata_tests {
    use super::*;

    #[test]
    fn public_stack_hashes_survive_only_in_metadata_and_registered_secrets_still_win() {
        let root = tempfile::tempdir().unwrap();
        let key = "8171eb680049f62ce8747d203f57c3f71de6eea308af6f995363896a1e47cc51";
        let digest = format!("sha256:{}", "b".repeat(64));
        let stack: crate::stack::StackSnapshot = serde_json::from_value(serde_json::json!({
            "daemon_pubkey":key,"relay_url":"ws://127.0.0.1:7000","wallet_endpoints":{},
            "daemon_image":{"image_id":format!("sha256:{}", "a".repeat(64)),
                "reference":format!("mostrop2p/mostro@{digest}"),"repository_digest":digest},
            "declared_limits":{"order_expiration_secs":300,"hold_invoice_expiry_secs":120,
                "buyer_invoice_deadline_secs":120,"payout_invoice_min_expiry_secs":300}
        }))
        .unwrap();
        let artifacts = RunArtifacts::create(root.path(), RunId::new(), Redactor::new()).unwrap();
        let value = serde_json::json!({"stack":stack,"stderr":key,"token":"private-wallet-token"});
        artifacts.add_secret(&Secret::new("private-wallet-token"));
        artifacts
            .write_metadata_json(files::ENVIRONMENT, &value, Some(&stack), None)
            .unwrap();
        let saved: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(artifacts.file(files::ENVIRONMENT).unwrap()).unwrap(),
        )
        .unwrap();
        assert_eq!(saved["stack"]["daemon_pubkey"], key);
        assert_eq!(saved["stack"]["daemon_image"]["repository_digest"], digest);
        assert_eq!(
            saved["stack"]["daemon_image"]["reference"],
            stack.daemon_image.reference
        );
        let restored: crate::stack::DaemonImage =
            serde_json::from_value(saved["stack"]["daemon_image"].clone()).unwrap();
        assert!(restored.validate().is_ok());
        assert_ne!(saved["stderr"], key);
        assert_ne!(saved["token"], "private-wallet-token");
        assert!(
            artifacts
                .write_metadata_json(files::RUN_LOG, &value, Some(&stack), None)
                .is_err()
        );
        artifacts.add_secret(&Secret::new(key));
        artifacts
            .write_metadata_json(files::SUMMARY, &value, Some(&stack), None)
            .unwrap();
        let text = std::fs::read_to_string(artifacts.file(files::SUMMARY).unwrap()).unwrap();
        assert!(!text.contains(key));
    }
}

fn restore_stack(
    tree: &mut serde_json::Value,
    stack: &crate::stack::StackSnapshot,
    redactor: &Redactor,
) -> io::Result<()> {
    for (pointer, public) in [
        ("/stack/daemon_pubkey", Some(stack.daemon_pubkey.as_str())),
        (
            "/stack/daemon_image/image_id",
            Some(stack.daemon_image.image_id.as_str()),
        ),
        (
            "/stack/daemon_image/repository_digest",
            stack.daemon_image.repository_digest.as_deref(),
        ),
    ] {
        let field = tree
            .pointer_mut(pointer)
            .ok_or_else(|| io::Error::other("missing stack metadata field"))?;
        *field = public
            .map(|value| serde_json::Value::String(redactor.redact_known(value)))
            .unwrap_or(serde_json::Value::Null);
    }
    if let Some((repository, digest)) = stack.daemon_image.reference.rsplit_once('@') {
        let reference = format!(
            "{}@{}",
            redactor.redact(repository),
            redactor.redact_known(digest)
        );
        let field = tree
            .pointer_mut("/stack/daemon_image/reference")
            .ok_or_else(|| io::Error::other("missing image reference field"))?;
        *field = serde_json::Value::String(redactor.redact_known(&reference));
    }
    Ok(())
}

fn restore_bundle(
    tree: &mut serde_json::Value,
    app_artifact: Option<&crate::fingerprint::BundleFingerprint>,
    redactor: &Redactor,
) -> io::Result<()> {
    if let Some(bundle) = app_artifact {
        if bundle.algorithm != "sha256-mortsom-bundle-v1"
            || bundle.files == 0
            || bundle.digest.len() != 64
            || !bundle.digest.bytes().all(|b| b.is_ascii_hexdigit())
        {
            return Err(io::Error::other("invalid bundle fingerprint"));
        }
        let field = tree
            .pointer_mut("/app_artifact/digest")
            .ok_or_else(|| io::Error::other("missing bundle fingerprint field"))?;
        *field = serde_json::Value::String(redactor.redact_known(&bundle.digest));
    }
    Ok(())
}
