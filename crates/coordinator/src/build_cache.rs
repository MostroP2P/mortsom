//! Managed-source app bundles kept across invocations and reused when the
//! exact same build is requested again.
use crate::fingerprint::{BundleFingerprint, CapturedBundle};
use crate::provenance::{ArtifactOrigin, BuildProvenance};
use mortsom_desktop_driver::Platform;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::io;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// Kept bundles, inside the ignored managed app working directory.
pub const DIR: &str = ".mortsom/app/bundles";
/// Bundles kept after a store; the least recently used beyond this are evicted.
pub const MAX_KEPT: usize = 4;

const FORMAT: &str = "mortsom-build-cache-v1";
const MANIFEST: &str = "manifest.json";
const BUNDLE: &str = "bundle";
const STAGING_PREFIX: &str = ".staging-";

/// Whether an invocation may reuse a kept bundle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Policy {
    /// Reuse a matching kept bundle; build and keep it otherwise.
    #[default]
    Reuse,
    /// Build even on a match, then replace the kept bundle.
    Rebuild,
}

/// Everything that decides a managed build's bytes; how the commit was
/// selected (branch or pull request) does not.
#[derive(Serialize)]
struct KeyInputs<'a> {
    format: &'a str,
    platform: Platform,
    revision: &'a str,
    app_version: &'a str,
    tools: &'a BTreeMap<String, String>,
    commands: &'a [Vec<String>],
}

/// Identity of everything that decides a managed build's bytes, or `None`
/// when the build cannot be reused (prebuilt, dirty or incomplete facts).
pub fn key(
    platform: Platform,
    provenance: &BuildProvenance,
    commands: &[Vec<String>],
) -> Option<String> {
    if provenance.origin != ArtifactOrigin::Source {
        return None;
    }
    let source = provenance.source_before.as_ref().filter(|s| !s.dirty)?;
    let inputs = KeyInputs {
        format: FORMAT,
        platform,
        revision: &source.revision,
        app_version: provenance.app_version.as_deref()?,
        tools: &provenance.tools,
        commands,
    };
    let encoded = serde_json::to_vec(&inputs).ok()?;
    Some(
        Sha256::digest(encoded)
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect(),
    )
}

/// A kept bundle whose content still matches its recorded fingerprint.
#[derive(Debug)]
pub struct KeptBuild {
    /// Root of the kept bundle files.
    pub root: PathBuf,
    /// The original build's facts, with origin `cached`.
    pub provenance: BuildProvenance,
    /// Recorded content identity, verified by the lookup.
    pub fingerprint: BundleFingerprint,
}

/// What an entry records next to its bundle copy.
#[derive(Serialize, Deserialize)]
struct Manifest {
    format: String,
    key: String,
    fingerprint: BundleFingerprint,
    provenance: BuildProvenance,
}

/// The directory of kept bundles, one entry per build key.
#[derive(Debug)]
pub struct BuildCache {
    root: PathBuf,
}

impl BuildCache {
    /// Opens the cache directory, creating it, and refuses a symlinked one.
    pub fn open(root: &Path) -> io::Result<Self> {
        std::fs::create_dir_all(root)?;
        if std::fs::symlink_metadata(root)?.file_type().is_symlink() {
            return Err(io::Error::other(
                "kept bundle directory must not be a symlink",
            ));
        }
        Ok(Self {
            root: root.canonicalize()?,
        })
    }

    /// The verified bundle kept under `key`, marked used at `now`. An entry
    /// that fails verification is discarded and reported as a miss.
    pub fn lookup(&self, key: &str, now: SystemTime) -> io::Result<Option<KeptBuild>> {
        let entry = self.entry(key)?;
        match std::fs::symlink_metadata(&entry) {
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error),
            Ok(metadata) if !metadata.is_dir() => {
                return Err(io::Error::other("kept bundle entry is not a directory"));
            }
            Ok(_) => {}
        }
        match verify(&entry, key) {
            Ok(kept) => {
                if let Err(error) = touch(&entry.join(MANIFEST), now) {
                    tracing::warn!(%error, "cannot record the kept app build's last use");
                }
                Ok(Some(kept))
            }
            Err(error) => {
                tracing::warn!(%error, "discarding a kept app bundle that failed verification");
                std::fs::remove_dir_all(&entry)?;
                Ok(None)
            }
        }
    }

    /// Keeps a copy of `bundle` under `key`, replacing any previous entry,
    /// then evicts the least recently used entries beyond [`MAX_KEPT`].
    pub fn store(
        &self,
        key: &str,
        bundle: &CapturedBundle,
        provenance: &BuildProvenance,
        now: SystemTime,
    ) -> io::Result<()> {
        let entry = self.entry(key)?;
        self.remove_staging()?;
        let staging = tempfile::Builder::new()
            .prefix(STAGING_PREFIX)
            .tempdir_in(&self.root)?;
        let kept = CapturedBundle::capture_persistent(bundle.root(), &staging.path().join(BUNDLE))?;
        if kept.fingerprint() != bundle.fingerprint() {
            return Err(io::Error::other(
                "kept bundle differs from the captured build",
            ));
        }
        let manifest = Manifest {
            format: FORMAT.into(),
            key: key.into(),
            fingerprint: bundle.fingerprint().clone(),
            provenance: provenance.clone(),
        };
        let path = staging.path().join(MANIFEST);
        std::fs::write(&path, serde_json::to_vec_pretty(&manifest)?)?;
        touch(&path, now)?;
        remove_entry(&entry)?;
        // A failed rename leaves the staging directory for the next store.
        std::fs::rename(staging.keep(), &entry)?;
        self.evict(MAX_KEPT)
    }

    /// Removes the bundle kept under `key`, if any.
    pub fn discard(&self, key: &str) -> io::Result<()> {
        remove_entry(&self.entry(key)?)
    }

    fn entry(&self, key: &str) -> io::Result<PathBuf> {
        if is_key(key) {
            Ok(self.root.join(key))
        } else {
            Err(io::Error::other(
                "build key must be a lowercase SHA-256 digest",
            ))
        }
    }

    /// Removes what an interrupted store left; the host lock excludes a live one.
    fn remove_staging(&self) -> io::Result<()> {
        for item in std::fs::read_dir(&self.root)? {
            let item = item?;
            if item
                .file_name()
                .to_string_lossy()
                .starts_with(STAGING_PREFIX)
            {
                remove_entry(&item.path())?;
            }
        }
        Ok(())
    }

    fn evict(&self, keep: usize) -> io::Result<()> {
        let mut entries = Vec::new();
        for item in std::fs::read_dir(&self.root)? {
            let item = item?;
            if !item.file_name().to_str().is_some_and(is_key) || !item.file_type()?.is_dir() {
                continue;
            }
            // An entry without a readable manifest is the least recently used.
            let used = std::fs::metadata(item.path().join(MANIFEST))
                .and_then(|metadata| metadata.modified())
                .unwrap_or(SystemTime::UNIX_EPOCH);
            entries.push((used, item.path()));
        }
        entries.sort_by_key(|(used, _)| std::cmp::Reverse(*used));
        // One entry that cannot be removed does not keep the others.
        let mut failure = None;
        for (_, path) in entries.into_iter().skip(keep) {
            if let Err(error) = std::fs::remove_dir_all(path) {
                failure.get_or_insert(error);
            }
        }
        failure.map_or(Ok(()), Err)
    }
}

fn is_key(name: &str) -> bool {
    name.len() == 64 && name.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

fn verify(entry: &Path, key: &str) -> io::Result<KeptBuild> {
    let manifest: Manifest = serde_json::from_slice(&std::fs::read(entry.join(MANIFEST))?)?;
    if manifest.format != FORMAT || manifest.key != key {
        return Err(io::Error::other(
            "kept bundle manifest does not match its key",
        ));
    }
    let root = entry.join(BUNDLE);
    CapturedBundle::resume(&root, manifest.fingerprint.clone())?;
    Ok(KeptBuild {
        root,
        provenance: BuildProvenance {
            origin: ArtifactOrigin::Cached,
            ..manifest.provenance
        },
        fingerprint: manifest.fingerprint,
    })
}

fn touch(path: &Path, now: SystemTime) -> io::Result<()> {
    std::fs::File::options()
        .write(true)
        .open(path)?
        .set_modified(now)
}

fn remove_entry(path: &Path) -> io::Result<()> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_dir() => std::fs::remove_dir_all(path),
        Ok(_) => std::fs::remove_file(path),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

#[cfg(test)]
#[path = "build_cache_tests.rs"]
mod tests;
