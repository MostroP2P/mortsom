//! Run-owned bundle storage, separate from mutable build outputs.
use super::*;

/// An owned snapshot retained until both actor sessions have stopped.
#[derive(Debug)]
pub struct CapturedBundle {
    root: std::path::PathBuf,
    _temporary: Option<tempfile::TempDir>,
    fingerprint: BundleFingerprint,
}

impl CapturedBundle {
    /// Copies a complete bundle without following links or special files.
    pub fn capture(source: &Path) -> io::Result<Self> {
        let expected = fingerprint(source, &RealBundleReader)?;
        let root = tempfile::Builder::new()
            .prefix("mortsom-bundle-")
            .tempdir()?;
        let entries = read_entries_with(open_root(source)?, |relative, file| {
            copy_file(root.path(), relative, file)
        })?;
        let captured = fingerprint_entries(entries)?;
        if captured != expected {
            return Err(io::Error::other("bundle changed during capture"));
        }
        Ok(Self {
            root: root.path().to_path_buf(),
            _temporary: Some(root),
            fingerprint: captured,
        })
    }

    /// Capture durable recovery bytes; the caller must explicitly remove them.
    pub fn capture_persistent(source: &Path, target: &Path) -> io::Result<Self> {
        use std::os::unix::fs::DirBuilderExt;
        let expected = fingerprint(source, &RealBundleReader)?;
        std::fs::DirBuilder::new().mode(0o700).create(target)?;
        let entries = read_entries_with(open_root(source)?, |relative, file| {
            copy_file(target, relative, file)
        })?;
        if fingerprint_entries(entries)? != expected {
            return Err(io::Error::other("bundle changed during persistent capture"));
        }
        for entry in walk_directories(target)? {
            std::fs::File::open(entry)?.sync_all()?;
        }
        std::fs::File::open(
            target
                .parent()
                .ok_or_else(|| io::Error::other("bundle has no parent"))?,
        )?
        .sync_all()?;
        Self::resume(target, expected)
    }
    /// Revalidate retained content without acquiring automatic deletion ownership.
    pub fn resume(root: &Path, fingerprint: BundleFingerprint) -> io::Result<Self> {
        let bundle = Self {
            root: root.to_path_buf(),
            _temporary: None,
            fingerprint,
        };
        bundle.verify()?;
        Ok(bundle)
    }

    /// Private directory containing the captured bundle.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Identity of the captured contents.
    pub fn fingerprint(&self) -> &BundleFingerprint {
        &self.fingerprint
    }

    /// Rechecks the owned bytes before a client can launch or serve them.
    pub fn verify(&self) -> io::Result<()> {
        if fingerprint(self.root(), &RealBundleReader)? != self.fingerprint {
            return Err(io::Error::other("captured app bundle was modified"));
        }
        Ok(())
    }
}

fn copy_file(root: &Path, relative: &Path, source: std::fs::File) -> io::Result<(u64, [u8; 32])> {
    use std::os::unix::fs::PermissionsExt;
    let mode = if source.metadata()?.permissions().mode() & 0o111 != 0 {
        0o500
    } else {
        0o400
    };
    let target = root.join(relative);
    std::fs::create_dir_all(
        target
            .parent()
            .ok_or_else(|| io::Error::other("bundle path has no parent"))?,
    )?;
    let mut output = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(target)?;
    let result = stream_file(source, Some(&mut output))?;
    output.set_permissions(std::fs::Permissions::from_mode(mode))?;
    output.sync_all()?;
    Ok(result)
}

fn walk_directories(root: &Path) -> io::Result<Vec<std::path::PathBuf>> {
    let mut directories = vec![root.to_path_buf()];
    let mut index = 0;
    while index < directories.len() {
        for entry in std::fs::read_dir(&directories[index])? {
            let entry = entry?;
            if entry.file_type()?.is_dir() {
                directories.push(entry.path());
            }
        }
        index += 1;
    }
    directories.reverse();
    Ok(directories)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn capture_keeps_executable_content_independent_of_later_source_builds() {
        let source = tempfile::tempdir().unwrap();
        let executable = source.path().join("mostro");
        std::fs::write(&executable, b"executable").unwrap();
        std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o755)).unwrap();
        std::fs::create_dir(source.path().join("data")).unwrap();
        std::fs::write(source.path().join("data/asset"), b"asset").unwrap();
        let expected = fingerprint(source.path(), &RealBundleReader).unwrap();
        let captured = CapturedBundle::capture(source.path()).unwrap();
        assert_eq!(captured.fingerprint(), &expected);
        assert_eq!(
            fingerprint(captured.root(), &RealBundleReader).unwrap(),
            expected
        );
        captured.verify().unwrap();
        std::fs::write(&executable, b"rebuilt").unwrap();
        assert_eq!(
            std::fs::read(captured.root().join("mostro")).unwrap(),
            b"executable"
        );
        let mode = std::fs::metadata(captured.root().join("mostro"))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o500);
        let mode = std::fs::metadata(captured.root().join("data/asset"))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o400);
        let owned = captured.root().to_path_buf();
        drop(captured);
        assert!(!owned.exists());
    }

    #[test]
    fn persistent_capture_survives_owner_drop_and_rejects_changed_bytes_on_resume() {
        let source = tempfile::tempdir().unwrap();
        std::fs::write(source.path().join("asset"), b"app").unwrap();
        let state = tempfile::tempdir().unwrap();
        let target = state.path().join("bundle");
        let captured = CapturedBundle::capture_persistent(source.path(), &target).unwrap();
        let identity = captured.fingerprint().clone();
        drop(captured);
        let restored = CapturedBundle::resume(&target, identity.clone()).unwrap();
        drop(restored);
        std::fs::set_permissions(target.join("asset"), std::fs::Permissions::from_mode(0o600))
            .unwrap();
        std::fs::write(target.join("asset"), b"changed").unwrap();
        assert!(CapturedBundle::resume(&target, identity).is_err());
    }

    #[test]
    fn capture_refuses_symlinked_content_instead_of_copying_external_files() {
        let source = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink("/etc/hostname", source.path().join("asset")).unwrap();
        assert!(CapturedBundle::capture(source.path()).is_err());
    }

    #[test]
    fn revalidation_detects_changes_to_the_owned_snapshot() {
        let source = tempfile::tempdir().unwrap();
        std::fs::write(source.path().join("asset"), b"original").unwrap();
        let captured = CapturedBundle::capture(source.path()).unwrap();
        let asset = captured.root().join("asset");
        std::fs::set_permissions(&asset, std::fs::Permissions::from_mode(0o600)).unwrap();
        std::fs::write(asset, b"tampered").unwrap();
        assert!(captured.verify().is_err());
    }
}
