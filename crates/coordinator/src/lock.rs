//! Single-run locking (spec Phase 6.5): one Mortsom run per host at a time.
//! Two runs would fight over emulator slots and, on the canary, could spend
//! twice; the lock is an advisory file lock that the OS releases when the
//! process ends, however it ends. Waiting is never done: a busy host is
//! reported.

use std::fs::{File, OpenOptions, TryLockError};
use std::path::{Path, PathBuf};

use thiserror::Error;

/// Lock file name inside the artifacts directory.
pub const LOCK_FILE: &str = ".mortsom-run.lock";

/// Why the lock was not taken.
#[derive(Debug, Error)]
pub enum LockError {
    /// Another process holds the lock.
    #[error("another mortsom run holds {0}; refusing to start a second run on this host")]
    Busy(PathBuf),
    /// The lock file could not be created, opened or locked.
    #[error("cannot lock {path}: {source}")]
    Io {
        /// Lock path.
        path: PathBuf,
        /// Cause.
        #[source]
        source: std::io::Error,
    },
}

/// The held lock; dropping it (or the process ending) releases the file.
#[derive(Debug)]
pub struct RunLock {
    file: File,
    path: PathBuf,
}

impl RunLock {
    /// Takes the exclusive run lock under `artifacts_dir` without waiting.
    pub fn acquire(artifacts_dir: &Path) -> Result<Self, LockError> {
        std::fs::create_dir_all(artifacts_dir).map_err(|source| LockError::Io {
            path: artifacts_dir.to_path_buf(),
            source,
        })?;
        let path = artifacts_dir.join(LOCK_FILE);
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(&path)
            .map_err(|source| LockError::Io {
                path: path.clone(),
                source,
            })?;
        match file.try_lock() {
            Ok(()) => Ok(Self { file, path }),
            Err(TryLockError::WouldBlock) => Err(LockError::Busy(path)),
            Err(TryLockError::Error(source)) => Err(LockError::Io { path, source }),
        }
    }

    /// Path of the lock file.
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for RunLock {
    fn drop(&mut self) {
        // Closing the file releases the lock as well; unlocking explicitly
        // keeps the release independent of descriptor reuse.
        let _ = self.file.unlock();
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn second_holder_is_refused_until_the_first_drops() {
        let dir = tempfile::tempdir().unwrap();
        let first = RunLock::acquire(dir.path()).unwrap();
        assert!(first.path().ends_with(LOCK_FILE));
        let busy = RunLock::acquire(dir.path()).unwrap_err();
        assert!(matches!(busy, LockError::Busy(_)), "{busy}");
        drop(first);
        RunLock::acquire(dir.path()).unwrap();
    }

    #[test]
    fn creates_the_artifacts_dir_when_missing() {
        let dir = tempfile::tempdir().unwrap();
        let nested = dir.path().join("a").join("b");
        let lock = RunLock::acquire(&nested).unwrap();
        assert!(lock.path().is_file());
    }
}
