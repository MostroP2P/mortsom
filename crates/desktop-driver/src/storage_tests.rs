#![allow(clippy::unwrap_used)]
use super::*;
use std::os::unix::fs::{MetadataExt, PermissionsExt, symlink};

#[test]
fn an_unverified_previous_runtime_prevents_profile_recovery() {
    let parent = tempfile::tempdir().unwrap();
    let path = parent.path().join("alice");
    ActorStorage::Fresh(path.clone())
        .prepare(Platform::Linux)
        .unwrap();
    std::fs::write(path.join(".mortsom-active-runtime"), b"active").unwrap();
    let failure = ActorStorage::Resume(path).prepare(Platform::Linux);
    assert!(
        failure.is_err(),
        "an unverified old process may still own this profile"
    );
}

#[test]
fn dropping_runtime_ownership_never_claims_its_processes_stopped() {
    let parent = tempfile::tempdir().unwrap();
    let path = parent.path().join("alice");
    ActorStorage::Fresh(path.clone())
        .prepare(Platform::Web)
        .unwrap();
    let active = ActiveRuntime::begin(&path).unwrap();
    assert_eq!(
        std::fs::metadata(path.join(ACTIVE)).unwrap().mode() & 0o777,
        0o600
    );
    assert!(ActiveRuntime::begin(&path).is_err());
    drop(active);
    assert!(ensure_stopped(&path).is_err());
    assert!(ActorStorage::Resume(path).prepare(Platform::Web).is_err());
}

#[test]
fn fresh_storage_is_private_and_cannot_be_claimed_twice() {
    let parent = tempfile::tempdir().unwrap();
    let path = parent.path().join("alice");
    ActorStorage::Fresh(path.clone())
        .prepare(Platform::Linux)
        .unwrap();
    assert_eq!(std::fs::metadata(&path).unwrap().mode() & 0o777, 0o700);
    assert_eq!(
        std::fs::metadata(path.join(MARKER)).unwrap().mode() & 0o777,
        0o600
    );
    assert!(ActorStorage::Fresh(path).prepare(Platform::Linux).is_err());
}

#[test]
fn fresh_storage_refuses_existing_content_without_altering_it() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("existing"), b"keep").unwrap();
    assert!(
        ActorStorage::Fresh(dir.path().into())
            .prepare(Platform::Web)
            .is_err()
    );
    assert_eq!(std::fs::read(dir.path().join("existing")).unwrap(), b"keep");
    assert!(!dir.path().join(MARKER).exists());
}

#[test]
fn resume_requires_the_existing_correct_platform_layout() {
    let parent = tempfile::tempdir().unwrap();
    let path = parent.path().join("alice");
    assert!(
        ActorStorage::Resume(path.clone())
            .prepare(Platform::Linux)
            .is_err()
    );
    assert!(!path.exists());
    ActorStorage::Fresh(path.clone())
        .prepare(Platform::Linux)
        .unwrap();
    assert!(
        ActorStorage::Resume(path.clone())
            .prepare(Platform::Web)
            .is_err()
    );
    ActorStorage::Resume(path.clone())
        .prepare(Platform::Linux)
        .unwrap();
    std::fs::remove_dir(path.join("data")).unwrap();
    assert!(
        ActorStorage::Resume(path.clone())
            .prepare(Platform::Linux)
            .is_err()
    );
    assert!(!path.join("data").exists());
}

#[test]
fn storage_rejects_symlinks_and_public_permissions() {
    let parent = tempfile::tempdir().unwrap();
    let target = parent.path().join("target");
    std::fs::create_dir(&target).unwrap();
    std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o700)).unwrap();
    let link = parent.path().join("link");
    symlink(&target, &link).unwrap();
    assert!(ActorStorage::Fresh(link).prepare(Platform::Web).is_err());
    std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o755)).unwrap();
    assert!(ActorStorage::Fresh(target).prepare(Platform::Web).is_err());
}

#[test]
fn resume_rejects_replaced_or_public_marker_and_symlinked_data() {
    let parent = tempfile::tempdir().unwrap();
    let path = parent.path().join("alice");
    ActorStorage::Fresh(path.clone())
        .prepare(Platform::Linux)
        .unwrap();
    let marker = path.join(MARKER);
    std::fs::set_permissions(&marker, std::fs::Permissions::from_mode(0o644)).unwrap();
    assert!(
        ActorStorage::Resume(path.clone())
            .prepare(Platform::Linux)
            .is_err()
    );
    std::fs::set_permissions(&marker, std::fs::Permissions::from_mode(0o600)).unwrap();
    std::fs::remove_dir(path.join("data")).unwrap();
    symlink(path.join("config"), path.join("data")).unwrap();
    assert!(
        ActorStorage::Resume(path.clone())
            .prepare(Platform::Linux)
            .is_err()
    );
    std::fs::remove_file(&marker).unwrap();
    symlink(path.join("config/marker"), &marker).unwrap();
    assert!(ActorStorage::Resume(path).prepare(Platform::Linux).is_err());
}

#[test]
fn a_verified_stop_clears_only_the_marker_of_its_own_platform() {
    let parent = tempfile::tempdir().unwrap();
    let path = parent.path().join("alice");
    ActorStorage::Fresh(path.clone())
        .prepare(Platform::Linux)
        .unwrap();
    let _left_behind = ActiveRuntime::begin(&path).unwrap();

    assert!(clear_verified_stop(&path, Platform::Cli).is_err());
    assert!(ensure_stopped(&path).is_err(), "another runtime's marker");
    clear_verified_stop(&path, Platform::Linux).unwrap();
    ensure_stopped(&path).unwrap();
    clear_verified_stop(&path, Platform::Linux).unwrap();
}

#[test]
fn a_verified_stop_needs_initialized_storage_at_an_absolute_path() {
    let parent = tempfile::tempdir().unwrap();
    let unclaimed = parent.path().join("bob");
    std::fs::DirBuilder::new()
        .mode(0o700)
        .create(&unclaimed)
        .unwrap();

    assert!(clear_verified_stop(Path::new("relative/alice"), Platform::Cli).is_err());
    assert!(clear_verified_stop(&unclaimed, Platform::Cli).is_err());
}

#[test]
fn each_platform_prepares_exactly_its_layout() {
    for platform in [Platform::Linux, Platform::Web, Platform::Cli] {
        let parent = tempfile::tempdir().unwrap();
        let path = parent.path().join("alice");
        ActorStorage::Fresh(path.clone()).prepare(platform).unwrap();

        let mut children: Vec<_> = std::fs::read_dir(&path)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().into_string().unwrap())
            .filter(|name| name != MARKER)
            .collect();
        children.sort();
        let mut expected: Vec<_> = layout(platform).iter().map(|s| s.to_string()).collect();
        expected.sort();
        assert_eq!(children, expected, "{platform}");
    }
}
