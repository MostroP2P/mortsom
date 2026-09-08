//! Run-owned bundle storage, separate from mutable build outputs.
use super::*;

/// An owned snapshot retained until both actor sessions have stopped.
#[derive(Debug)]
pub struct CapturedBundle {
    root: tempfile::TempDir,
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
            root,
            fingerprint: captured,
        })
    }

    /// Private directory containing the captured bundle.
    pub fn root(&self) -> &Path {
        self.root.path()
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
    Ok(result)
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
