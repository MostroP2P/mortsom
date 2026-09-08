//! Private JSON staging and durable publication of report directory entries.
use std::{io, path::Path};

pub(super) fn create_directory(path: &Path) -> io::Result<()> {
    create_with(path, &RealDirectorySync)
}

trait DirectorySync {
    fn sync(&self, path: &Path) -> io::Result<()>;
}

struct RealDirectorySync;
impl DirectorySync for RealDirectorySync {
    fn sync(&self, path: &Path) -> io::Result<()> {
        std::fs::File::open(path)?.sync_all()
    }
}

fn create_with(path: &Path, sync: &dyn DirectorySync) -> io::Result<()> {
    use std::os::unix::fs::DirBuilderExt;
    let path = std::path::absolute(path)?;
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(&path)?;
    // Include pre-existing ancestors: they may have been created by an earlier
    // attempt whose parent sync failed. A retry must close that durability gap.
    for ancestor in path.ancestors() {
        sync.sync(ancestor)?;
    }
    Ok(())
}

pub(super) fn atomic_json(path: &Path, json: &str) -> io::Result<()> {
    use std::io::Write;
    let parent = path
        .parent()
        .ok_or_else(|| io::Error::other("JSON artifact has no parent"))?;
    let mut staged = tempfile::NamedTempFile::new_in(parent)?;
    staged.write_all(json.as_bytes())?;
    staged.as_file().sync_all()?;
    staged.persist(path).map_err(|error| error.error)?;
    std::fs::File::open(parent)?.sync_all()
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use std::{path::PathBuf, sync::Mutex};
    #[derive(Default)]
    struct Sync {
        calls: Mutex<Vec<PathBuf>>,
    }
    impl DirectorySync for Sync {
        fn sync(&self, path: &Path) -> io::Result<()> {
            self.calls.lock().unwrap().push(path.to_path_buf());
            Ok(())
        }
    }
    #[test]
    fn fresh_directory_entries_are_synced_through_the_existing_ancestors() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("artifacts/run/alice/steps");
        let sync = Sync::default();
        create_with(&path, &sync).unwrap();
        assert_eq!(
            *sync.calls.lock().unwrap(),
            path.ancestors().map(Path::to_path_buf).collect::<Vec<_>>()
        );
        sync.calls.lock().unwrap().clear();
        create_with(&path, &sync).unwrap();
        assert_eq!(
            *sync.calls.lock().unwrap(),
            path.ancestors().map(Path::to_path_buf).collect::<Vec<_>>()
        );
    }
}
