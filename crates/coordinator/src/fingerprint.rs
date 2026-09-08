//! Content identity for an entire app bundle, independent of checkout location.
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::io::Read;
use std::{io, path::Path};

mod capture;
pub use capture::CapturedBundle;

/// Versioned hash of sorted relative paths, file lengths and content hashes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BundleFingerprint {
    /// Hash algorithm and framing version.
    pub algorithm: String,
    /// SHA-256 of the complete framed manifest.
    pub digest: String,
    /// Number of regular files included.
    pub files: u64,
    /// Total file content size.
    pub bytes: u64,
}

/// File-system boundary used to read bundle content facts.
pub trait BundleReader {
    /// Reads all regular files, rejecting links and special files.
    fn entries(&self, root: &Path) -> io::Result<Vec<BundleEntry>>;
}

/// One public content fact; no source or credential content is retained.
pub struct BundleEntry {
    /// UTF-8 path relative to the artifact root.
    pub path: String,
    /// File content length.
    pub bytes: u64,
    /// SHA-256 of the file content.
    pub hash: [u8; 32],
}

/// Computes a deterministic fingerprint from an injected file reader.
pub fn fingerprint(root: &Path, reader: &dyn BundleReader) -> io::Result<BundleFingerprint> {
    fingerprint_entries(reader.entries(root)?)
}

fn fingerprint_entries(mut entries: Vec<BundleEntry>) -> io::Result<BundleFingerprint> {
    entries.sort_by(|a, b| a.path.cmp(&b.path));
    if entries.is_empty() || entries.windows(2).any(|pair| pair[0].path == pair[1].path) {
        return Err(io::Error::other("empty or duplicate bundle manifest"));
    }
    let mut hasher = Sha256::new();
    hasher.update(b"mortsom-bundle-v1\0");
    let mut bytes = 0_u64;
    for entry in &entries {
        if entry.path.is_empty()
            || !Path::new(&entry.path)
                .components()
                .all(|part| matches!(part, std::path::Component::Normal(_)))
        {
            return Err(io::Error::other(
                "bundle path must be relative and contained",
            ));
        }
        bytes = bytes
            .checked_add(entry.bytes)
            .ok_or_else(|| io::Error::other("bundle size overflow"))?;
        hasher.update((entry.path.len() as u64).to_be_bytes());
        hasher.update(entry.path.as_bytes());
        hasher.update(entry.bytes.to_be_bytes());
        hasher.update(entry.hash);
    }
    Ok(BundleFingerprint {
        algorithm: "sha256-mortsom-bundle-v1".into(),
        digest: hasher
            .finalize()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect(),
        files: entries.len() as u64,
        bytes,
    })
}

/// Reads real bundle files using a bounded buffer; never follows a symlink.
pub struct RealBundleReader;

impl BundleReader for RealBundleReader {
    fn entries(&self, root: &Path) -> io::Result<Vec<BundleEntry>> {
        read_entries(open_root(root)?)
    }
}

fn open_root(root: &Path) -> io::Result<std::fs::File> {
    use rustix::fs::{Mode, OFlags, open, openat};
    let flags = OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC;
    let mut directory = open(
        if root.is_absolute() { "/" } else { "." },
        flags,
        Mode::empty(),
    )?;
    for component in root.components() {
        match component {
            std::path::Component::Normal(name) => {
                directory = openat(&directory, name, flags, Mode::empty())?
            }
            std::path::Component::RootDir | std::path::Component::CurDir => {}
            _ => {
                return Err(io::Error::other(
                    "bundle root must not traverse parent directories",
                ));
            }
        }
    }
    Ok(directory.into())
}

fn open_entry(directory: &std::fs::File, name: &std::ffi::CStr) -> io::Result<std::fs::File> {
    use rustix::fs::{Mode, OFlags, openat};
    let file: std::fs::File = openat(
        directory,
        name,
        OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
        Mode::empty(),
    )?
    .into();
    let metadata = file.metadata()?;
    if !metadata.is_file() && !metadata.is_dir() {
        return Err(io::Error::other("bundle contains a special file"));
    }
    Ok(file)
}

fn read_entries(root: std::fs::File) -> io::Result<Vec<BundleEntry>> {
    read_entries_with(root, |_, file| hash_file(file))
}

fn read_entries_with(
    root: std::fs::File,
    mut consume: impl FnMut(&Path, std::fs::File) -> io::Result<(u64, [u8; 32])>,
) -> io::Result<Vec<BundleEntry>> {
    let mut directories = vec![(std::path::PathBuf::new(), root)];
    let mut entries = Vec::new();
    while let Some((relative, directory)) = directories.pop() {
        for item in rustix::fs::Dir::read_from(&directory)? {
            let item = item?;
            let name = item.file_name().to_str().map_err(io::Error::other)?;
            if matches!(name, "." | "..") {
                continue;
            }
            let path = relative.join(name);
            if path.components().count() > 64 {
                return Err(io::Error::other("bundle directory depth exceeds 64"));
            }
            let file = open_entry(&directory, item.file_name())?;
            if file.metadata()?.is_dir() {
                directories.push((path, file));
            } else {
                let (bytes, hash) = consume(&path, file)?;
                let path = path
                    .to_str()
                    .ok_or_else(|| io::Error::other("bundle filename is not UTF-8"))?
                    .into();
                entries.push(BundleEntry { path, bytes, hash });
            }
        }
    }
    Ok(entries)
}

fn hash_file(file: std::fs::File) -> io::Result<(u64, [u8; 32])> {
    stream_file(file, None)
}

fn stream_file(
    mut file: std::fs::File,
    mut copy: Option<&mut std::fs::File>,
) -> io::Result<(u64, [u8; 32])> {
    let before = file.metadata()?;
    if !before.is_file() {
        return Err(io::Error::other("bundle entry is not a regular file"));
    }
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 65536];
    let mut bytes = 0_u64;
    loop {
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        bytes += count as u64;
        hasher.update(&buffer[..count]);
        if let Some(output) = &mut copy {
            std::io::Write::write_all(output, &buffer[..count])?;
        }
    }
    let after = file.metadata()?;
    if before.len() != bytes || after.len() != bytes || before.modified()? != after.modified()? {
        return Err(io::Error::other("bundle changed while being fingerprinted"));
    }
    Ok((bytes, hasher.finalize().into()))
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    struct Fake(Vec<(&'static str, u64, u8)>);
    impl BundleReader for Fake {
        fn entries(&self, _: &Path) -> io::Result<Vec<BundleEntry>> {
            Ok(self
                .0
                .iter()
                .map(|(path, bytes, hash)| BundleEntry {
                    path: (*path).into(),
                    bytes: *bytes,
                    hash: [*hash; 32],
                })
                .collect())
        }
    }
    #[test]
    fn hash_is_location_and_traversal_independent_but_covers_names_lengths_and_bytes() {
        let base = fingerprint(Path::new("/a"), &Fake(vec![("a", 1, 1), ("b", 2, 2)])).unwrap();
        assert_eq!(
            base,
            fingerprint(Path::new("/other"), &Fake(vec![("b", 2, 2), ("a", 1, 1)])).unwrap()
        );
        assert_eq!((base.files, base.bytes), (2, 3));
        for changed in [
            vec![("a", 1, 1), ("c", 2, 2)],
            vec![("a", 1, 1), ("b", 3, 2)],
            vec![("a", 1, 1), ("b", 2, 3)],
        ] {
            assert_ne!(
                base.digest,
                fingerprint(Path::new("/a"), &Fake(changed)).unwrap().digest
            );
        }
    }
    #[test]
    fn empty_duplicate_or_escaping_manifests_are_rejected() {
        for files in [
            vec![],
            vec![("a", 1, 1), ("a", 2, 2)],
            vec![("../outside", 1, 1)],
            vec![("/absolute", 1, 1)],
        ] {
            assert!(fingerprint(Path::new("/a"), &Fake(files)).is_err());
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod filesystem_tests {
    use super::*;

    #[test]
    fn real_reader_hashes_contents_and_rejects_file_and_directory_links() {
        let root = tempfile::tempdir().unwrap();
        let nested = root.path().join("data");
        std::fs::create_dir(&nested).unwrap();
        std::fs::write(nested.join("asset"), b"abc").unwrap();
        let first = fingerprint(root.path(), &RealBundleReader).unwrap();
        assert_eq!((first.files, first.bytes), (1, 3));
        std::fs::write(nested.join("asset"), b"abd").unwrap();
        assert_ne!(
            first.digest,
            fingerprint(root.path(), &RealBundleReader).unwrap().digest
        );
        let link = root.path().join("link");
        for target in [nested.join("asset"), nested] {
            std::os::unix::fs::symlink(target, &link).unwrap();
            assert!(fingerprint(root.path(), &RealBundleReader).is_err());
            std::fs::remove_file(&link).unwrap();
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod replacement_tests {
    use super::*;

    #[test]
    fn replacing_an_opened_directory_with_a_link_cannot_redirect_the_scan() {
        let parent = tempfile::tempdir().unwrap();
        let bundle = parent.path().join("bundle");
        let outside = parent.path().join("outside");
        std::fs::create_dir(&bundle).unwrap();
        std::fs::create_dir(&outside).unwrap();
        std::fs::write(bundle.join("original"), b"app").unwrap();
        std::fs::write(outside.join("private"), b"must not be read").unwrap();
        let directory = open_root(&bundle).unwrap();
        std::fs::rename(&bundle, parent.path().join("old-bundle")).unwrap();
        std::os::unix::fs::symlink(&outside, &bundle).unwrap();
        let entries = read_entries(directory).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].path, "original");
        assert!(open_root(&bundle).is_err());
    }

    #[test]
    fn a_leaf_replaced_with_a_fifo_is_rejected_without_waiting_for_a_writer() {
        let root = tempfile::tempdir().unwrap();
        let directory = open_root(root.path()).unwrap();
        std::fs::write(root.path().join("asset"), b"app").unwrap();
        let mut entries = rustix::fs::Dir::read_from(&directory).unwrap();
        assert!(entries.any(|entry| entry.unwrap().file_name() == c"asset"));
        std::fs::remove_file(root.path().join("asset")).unwrap();
        rustix::fs::mknodat(
            &directory,
            c"asset",
            rustix::fs::FileType::Fifo,
            rustix::fs::Mode::RUSR | rustix::fs::Mode::WUSR,
            0,
        )
        .unwrap();
        assert!(open_entry(&directory, c"asset").is_err());
    }
}
