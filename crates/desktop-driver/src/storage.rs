//! Caller-owned actor storage retained until verified recovery or cleanup.
use crate::Platform;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};

const MARKER: &str = ".mortsom-storage";
const ACTIVE: &str = ".mortsom-active-runtime";

/// Refuses storage whose previous runtime did not record a verified shutdown.
/// Absence proves only the driver's recorded lifecycle, never an app identity.
pub fn ensure_stopped(root: &Path) -> io::Result<()> {
    validate_path(root)?;
    validate_directory(root)?;
    match fs::symlink_metadata(root.join(ACTIVE)) {
        Err(cause) if cause.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(cause) => Err(cause),
        Ok(_) => Err(invalid(
            "unverified prior shutdown; reconcile the previous runtime before recovery",
        )),
    }
}

/// Deliberately has no automatic cleanup: coordinator death is not proof that
/// its child processes stopped. Only the runtime's verified stop clears it.
pub(crate) struct ActiveRuntime {
    root: PathBuf,
}
impl ActiveRuntime {
    pub(crate) fn begin(root: &Path) -> io::Result<Self> {
        ensure_stopped(root)?;
        let mut marker = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(root.join(ACTIVE))?;
        marker.write_all(b"mortsom-active-runtime-v1\n")?;
        marker.sync_all()?;
        File::open(root)?.sync_all()?;
        Ok(Self {
            root: root.to_path_buf(),
        })
    }

    pub(crate) fn stopped(&self) -> io::Result<()> {
        match fs::remove_file(self.root.join(ACTIVE)) {
            Ok(()) => {}
            Err(cause) if cause.kind() == io::ErrorKind::NotFound => {}
            Err(cause) => return Err(cause),
        }
        File::open(&self.root)?.sync_all()
    }
}

/// Persistent actor storage. The coordinator owns its eventual removal.
/// Paths must be absolute, canonical, private directories owned by this user.
/// A resume marker proves a storage layout, not an application identity; the
/// coordinator must verify the public identity before any recovery action.
#[derive(Debug, Clone)]
pub enum ActorStorage {
    /// Initialize an absent or empty private directory exactly once.
    Fresh(PathBuf),
    /// Reopen an existing initialized directory without clearing its contents.
    Resume(PathBuf),
}

impl ActorStorage {
    pub(crate) fn is_resume(&self) -> bool {
        matches!(self, Self::Resume(_))
    }

    pub(crate) fn prepare(&self, platform: Platform) -> io::Result<PathBuf> {
        let path = match self {
            Self::Fresh(path) | Self::Resume(path) => path,
        };
        let layout = match platform {
            Platform::Linux => &["data", "config"][..],
            Platform::Web => &["profile"][..],
            Platform::Android => return Err(invalid("desktop storage cannot contain Android")),
        };
        let marker = format!("mortsom-actor-storage-v1\n{platform}\n");
        validate_path(path)?;
        if self.is_resume() {
            ensure_stopped(path)?;
            validate_marker(path, &marker)?;
            for child in layout {
                validate_directory(&path.join(child))?;
            }
        } else {
            initialize(path, layout, &marker)?;
        }
        Ok(path.clone())
    }
}

fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message)
}

fn validate_path(path: &Path) -> io::Result<()> {
    if !path.is_absolute() {
        return Err(invalid("actor storage must use an absolute canonical path"));
    }
    let parent = path
        .parent()
        .ok_or_else(|| invalid("actor storage needs a parent"))?;
    if parent.canonicalize()? != parent {
        return Err(invalid("actor storage parent must be canonical"));
    }
    Ok(())
}

fn validate_directory(path: &Path) -> io::Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(invalid(
            "actor storage must be a directory, never a symbolic link",
        ));
    }
    if metadata.mode() & 0o777 != 0o700 || metadata.uid() != fs::metadata("/proc/self")?.uid() {
        return Err(invalid(
            "actor storage must be owned by this user with mode 0700",
        ));
    }
    Ok(())
}

fn validate_marker(path: &Path, expected: &str) -> io::Result<()> {
    let marker = path.join(MARKER);
    let metadata = fs::symlink_metadata(&marker)?;
    if !metadata.is_file()
        || metadata.file_type().is_symlink()
        || metadata.mode() & 0o777 != 0o600
        || metadata.uid() != fs::metadata("/proc/self")?.uid()
        || metadata.len() > 128
    {
        return Err(invalid(
            "actor storage marker is not a private owned regular file",
        ));
    }
    if fs::read_to_string(marker)? != expected {
        return Err(invalid(
            "actor storage marker does not match the requested platform",
        ));
    }
    Ok(())
}

fn initialize(path: &Path, layout: &[&str], marker: &str) -> io::Result<()> {
    match fs::DirBuilder::new().mode(0o700).create(path) {
        Ok(()) => {}
        Err(cause) if cause.kind() == io::ErrorKind::AlreadyExists => {}
        Err(cause) => return Err(cause),
    }
    validate_directory(path)?;
    if fs::read_dir(path)?.next().transpose()?.is_some() {
        return Err(invalid(
            "fresh actor storage is not empty; use explicit recovery",
        ));
    }
    let mut claim = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path.join(MARKER))?;
    claim.write_all(marker.as_bytes())?;
    claim.sync_all()?;
    for child in layout {
        let child = path.join(child);
        fs::DirBuilder::new().mode(0o700).create(&child)?;
        File::open(child)?.sync_all()?;
    }
    File::open(path)?.sync_all()?;
    File::open(
        path.parent()
            .ok_or_else(|| invalid("actor storage needs a parent"))?,
    )?
    .sync_all()
}

#[cfg(test)]
#[path = "storage_tests.rs"]
mod tests;
