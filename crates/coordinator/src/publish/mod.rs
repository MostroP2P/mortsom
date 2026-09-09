//! `mortsom publish <run>` (spec §11, Phase 6.4): builds the sanitized
//! bundle of a run for CI publication. Text evidence goes through the
//! redactor a second time (defense in depth), screenshots follow an
//! explicit policy because pixels cannot be scrubbed, and a manifest lists
//! every file, the retry classification per scenario and how many files
//! still contained a configured secret before this pass. A secret that
//! survives redaction aborts the publication.

use std::path::{Path, PathBuf};

use mortsom_common::{Lane, Redactor, Secret};
use mortsom_scenario_kit::outcome::Outcome;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tracing::{info, warn};

use crate::artifacts::files;
use crate::config::Config;
use crate::run::RunSummary;

/// Manifest file name inside the bundle.
pub const MANIFEST_FILE: &str = "publish.json";
/// Extensions treated as text and redacted.
const TEXT_EXTENSIONS: [&str; 7] = ["json", "jsonl", "md", "txt", "log", "xml", "toml"];
/// Extensions treated as images.
const IMAGE_EXTENSIONS: [&str; 2] = ["png", "jpg"];

/// What to do with screenshots.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum ScreenshotPolicy {
    /// Copy every screenshot.
    #[default]
    All,
    /// Copy none (canary default: pixels cannot be redacted).
    None,
}

impl std::str::FromStr for ScreenshotPolicy {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "all" => Ok(Self::All),
            "none" => Ok(Self::None),
            other => Err(format!("unknown screenshot policy `{other}` (all|none)")),
        }
    }
}

/// Publication options.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct PublishOptions {
    /// Screenshot policy.
    pub screenshots: ScreenshotPolicy,
}

/// Why publication failed.
#[derive(Debug, Error)]
pub enum PublishError {
    /// The run directory does not exist.
    #[error("run directory {0} does not exist")]
    MissingRun(PathBuf),
    /// Filesystem failure.
    #[error("{path}: {source}")]
    Io {
        /// Path involved.
        path: PathBuf,
        /// Cause.
        #[source]
        source: std::io::Error,
    },
    /// A configured secret survived redaction; nothing was published.
    #[error("{path} still contains a configured secret after redaction; bundle discarded")]
    Leak {
        /// Offending file.
        path: PathBuf,
    },
}

/// One published file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PublishedFile {
    /// Path relative to the bundle root.
    pub path: String,
    /// Bytes written.
    pub bytes: u64,
    /// `text` or `image`.
    pub kind: String,
    /// True when this pass changed the text (the run's own redaction had
    /// missed something).
    pub redacted_here: bool,
}

/// Retry classification of one scenario (spec §10).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScenarioClassification {
    /// Scenario name.
    pub name: String,
    /// `passed`, `failed` or `skipped`.
    pub outcome: String,
    /// Failure category when failed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub category: Option<String>,
    /// Attempts made.
    pub attempts: u32,
    /// Seconds of the last attempt.
    pub duration_secs: u64,
    /// Redacted detail.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

/// The bundle manifest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PublishManifest {
    /// Run id.
    pub run_id: String,
    /// Lane, when `summary.json` was readable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lane: Option<Lane>,
    /// Whether the run passed, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub passed: Option<bool>,
    /// Screenshot policy applied.
    pub screenshots: ScreenshotPolicy,
    /// Files in the bundle.
    pub files: Vec<PublishedFile>,
    /// Files left out (unknown type or excluded screenshots).
    pub skipped: Vec<String>,
    /// Per-scenario classification.
    pub scenarios: Vec<ScenarioClassification>,
    /// Text files this pass had to redact further.
    pub redacted_here: usize,
}

impl PublishManifest {
    /// Human-readable rendering.
    pub fn render(&self) -> String {
        let mut out = format!(
            "published run {} ({} files, {} skipped, screenshots: {:?})\n",
            self.run_id,
            self.files.len(),
            self.skipped.len(),
            self.screenshots
        );
        if self.redacted_here > 0 {
            out.push_str(&format!(
                "  WARNING: {} file(s) still contained secret-shaped text before this pass\n",
                self.redacted_here
            ));
        }
        for s in &self.scenarios {
            out.push_str(&format!(
                "  {:<32} {:<8} attempts={} {}s{}\n",
                s.name,
                s.outcome,
                s.attempts,
                s.duration_secs,
                s.category
                    .as_deref()
                    .map(|c| format!(" [{c}]"))
                    .unwrap_or_default()
            ));
        }
        out
    }
}

/// Publishes `run_id` from the configured artifacts directory into `out`.
pub fn publish_run(
    config: &Config,
    run_id: &str,
    out: &Path,
    options: PublishOptions,
) -> Result<PublishManifest, PublishError> {
    publish_artifacts(
        &crate::config::PublishSettings::from(config),
        run_id,
        out,
        options,
    )
}

/// Sanitizes existing evidence using only artifact settings and known secrets.
pub fn publish_artifacts(
    settings: &crate::config::PublishSettings,
    run_id: &str,
    out: &Path,
    options: PublishOptions,
) -> Result<PublishManifest, PublishError> {
    let run_dir = Path::new(&settings.artifacts.artifacts_dir).join(run_id);
    if !run_dir.is_dir() {
        return Err(PublishError::MissingRun(run_dir));
    }
    let redactor = Redactor::new().with_secrets(&settings.secrets);
    let secrets = settings.secrets.iter().collect::<Vec<_>>();
    std::fs::create_dir_all(out).map_err(|source| PublishError::Io {
        path: out.to_path_buf(),
        source,
    })?;
    let mut manifest = PublishManifest {
        run_id: run_id.to_owned(),
        lane: None,
        passed: None,
        screenshots: options.screenshots,
        files: Vec::new(),
        skipped: Vec::new(),
        scenarios: Vec::new(),
        redacted_here: 0,
    };
    let mut paths = Vec::new();
    collect(&run_dir, &run_dir, &mut paths)?;
    paths.sort();
    for relative in paths {
        copy_one(
            &run_dir,
            out,
            &relative,
            &redactor,
            &secrets,
            options,
            &mut manifest,
        )
        .map_err(|e| discard_bundle_on_leak(out, e))?;
    }
    classify(&run_dir, &redactor, &secrets, &mut manifest)
        .map_err(|e| discard_bundle_on_leak(out, e))?;
    let json = serde_json::to_string_pretty(&manifest).unwrap_or_default();
    std::fs::write(out.join(MANIFEST_FILE), json).map_err(|source| PublishError::Io {
        path: out.join(MANIFEST_FILE),
        source,
    })?;
    info!(run = run_id, files = manifest.files.len(), out = %out.display(), "run published");
    Ok(manifest)
}

/// A leak aborts publication, and [`PublishError::Leak`] promises a
/// discarded bundle: whatever was copied before the gate fired is removed so
/// no partially redacted evidence is left behind. Other errors keep the
/// directory for inspection. Removal is best effort; failing to remove must
/// not mask the leak itself.
fn discard_bundle_on_leak(out: &Path, error: PublishError) -> PublishError {
    if matches!(error, PublishError::Leak { .. })
        && let Err(e) = std::fs::remove_dir_all(out)
    {
        warn!(out = %out.display(), error = %e, "cannot remove the partial bundle after a leak");
    }
    error
}

fn collect(root: &Path, dir: &Path, out: &mut Vec<PathBuf>) -> Result<(), PublishError> {
    let entries = std::fs::read_dir(dir).map_err(|source| PublishError::Io {
        path: dir.to_path_buf(),
        source,
    })?;
    for entry in entries {
        let entry = entry.map_err(|source| PublishError::Io {
            path: dir.to_path_buf(),
            source,
        })?;
        let path = entry.path();
        let kind = entry.file_type().map_err(|source| PublishError::Io {
            path: path.clone(),
            source,
        })?;
        // A symlink could point outside the run directory; evidence is
        // never a link, so links are skipped rather than followed.
        if kind.is_symlink() {
            warn!(path = %path.display(), "symlink in run directory skipped");
            continue;
        }
        if kind.is_dir() {
            collect(root, &path, out)?;
        } else if let Ok(relative) = path.strip_prefix(root) {
            out.push(relative.to_path_buf());
        }
    }
    Ok(())
}

fn copy_one(
    run_dir: &Path,
    out: &Path,
    relative: &Path,
    redactor: &Redactor,
    secrets: &[&Secret],
    options: PublishOptions,
    manifest: &mut PublishManifest,
) -> Result<(), PublishError> {
    let source = run_dir.join(relative);
    let target = out.join(relative);
    let shown = relative.to_string_lossy().into_owned();
    let ext = relative
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    if TEXT_EXTENSIONS.contains(&ext.as_str()) {
        let raw = std::fs::read(&source).map_err(|source| PublishError::Io {
            path: relative.to_path_buf(),
            source,
        })?;
        let text = String::from_utf8_lossy(&raw);
        let clean = redactor.redact(&text);
        if secrets
            .iter()
            .any(|s| !s.expose().is_empty() && clean.contains(s.expose()))
        {
            return Err(PublishError::Leak {
                path: relative.to_path_buf(),
            });
        }
        let redacted_here = clean != text;
        if redacted_here {
            warn!(file = %shown, "publish pass redacted text the run had left in place");
            manifest.redacted_here += 1;
        }
        write(&target, clean.as_bytes())?;
        manifest.files.push(PublishedFile {
            path: shown,
            bytes: clean.len() as u64,
            kind: "text".into(),
            redacted_here,
        });
        return Ok(());
    }
    if IMAGE_EXTENSIONS.contains(&ext.as_str()) {
        if options.screenshots == ScreenshotPolicy::None {
            manifest.skipped.push(shown);
            return Ok(());
        }
        let bytes = std::fs::read(&source).map_err(|source| PublishError::Io {
            path: relative.to_path_buf(),
            source,
        })?;
        write(&target, &bytes)?;
        manifest.files.push(PublishedFile {
            path: shown,
            bytes: bytes.len() as u64,
            kind: "image".into(),
            redacted_here: false,
        });
        return Ok(());
    }
    manifest.skipped.push(shown);
    Ok(())
}

fn write(target: &Path, bytes: &[u8]) -> Result<(), PublishError> {
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent).map_err(|source| PublishError::Io {
            path: parent.to_path_buf(),
            source,
        })?;
    }
    std::fs::write(target, bytes).map_err(|source| PublishError::Io {
        path: target.to_path_buf(),
        source,
    })
}

/// Fills lane, verdict and per-scenario classification from `summary.json`
/// when it is a run summary; other commands' summaries leave them empty.
fn classify(
    run_dir: &Path,
    redactor: &Redactor,
    secrets: &[&Secret],
    manifest: &mut PublishManifest,
) -> Result<(), PublishError> {
    let Ok(text) = std::fs::read_to_string(run_dir.join(files::SUMMARY)) else {
        return Ok(());
    };
    let Ok(summary) = serde_json::from_str::<RunSummary>(&text) else {
        return Ok(());
    };
    manifest.lane = Some(summary.lane);
    manifest.passed = Some(summary.passed);
    // Free text from the summary goes through the same redaction and leak
    // gate as file bodies; the manifest is published too.
    let scrub = |text: &str| -> Result<String, PublishError> {
        let clean = redactor.redact(text);
        if secrets
            .iter()
            .any(|s| !s.expose().is_empty() && clean.contains(s.expose()))
        {
            return Err(PublishError::Leak {
                path: PathBuf::from(files::SUMMARY),
            });
        }
        Ok(clean)
    };
    let mut scenarios = Vec::with_capacity(summary.results.len());
    for r in &summary.results {
        let (outcome, category, detail) = match &r.outcome {
            Outcome::Passed => ("passed", None, None),
            Outcome::Failed { category, message } => {
                ("failed", Some(category.to_string()), Some(scrub(message)?))
            }
            Outcome::Skipped { reason } => ("skipped", None, Some(scrub(reason)?)),
        };
        scenarios.push(ScenarioClassification {
            name: scrub(&r.name)?,
            outcome: outcome.into(),
            category,
            attempts: r.attempts,
            duration_secs: r.duration.as_secs(),
            detail,
        });
    }
    manifest.scenarios = scenarios;
    Ok(())
}

#[cfg(test)]
mod tests;
