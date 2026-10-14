#![allow(clippy::unwrap_used)]
use super::*;
use crate::provenance::{ArtifactOrigin, SourceIdentity};
use std::collections::BTreeMap;
use std::time::Duration;

fn identity(revision: char, dirty: bool) -> Option<SourceIdentity> {
    Some(SourceIdentity {
        revision: revision.to_string().repeat(40),
        dirty,
    })
}

fn provenance() -> BuildProvenance {
    BuildProvenance {
        origin: ArtifactOrigin::Source,
        remote_selection: None,
        source_before: identity('a', false),
        source_after: identity('a', false),
        app_version: Some("2.0.16".into()),
        tools: BTreeMap::from([
            ("flutter".into(), "Flutter 3.41.9".into()),
            ("rustc".into(), "rustc 1.99.0".into()),
        ]),
        commands: commands(7000),
    }
}

fn commands(relay_port: u16) -> Vec<Vec<String>> {
    vec![
        vec!["env".into(), "--chdir=<source>".into(), "flutter".into()],
        vec![
            "env".into(),
            "--chdir=<source>".into(),
            format!("--dart-define=MORTSOM_RELAYS=ws://127.0.0.1:{relay_port}"),
        ],
    ]
}

fn at(seconds: u64) -> SystemTime {
    SystemTime::UNIX_EPOCH + Duration::from_secs(1_800_000_000 + seconds)
}

fn bundle(content: &[u8]) -> CapturedBundle {
    let source = tempfile::tempdir().unwrap();
    std::fs::create_dir(source.path().join("lib")).unwrap();
    std::fs::write(source.path().join("mostro"), b"executable").unwrap();
    std::fs::write(source.path().join("lib/libapp.so"), content).unwrap();
    CapturedBundle::capture(source.path()).unwrap()
}

fn key_for(seed: u8) -> String {
    format!("{seed:02x}").repeat(32)
}

#[test]
fn key_is_stable_and_changes_with_every_input_that_decides_the_bytes() {
    let base = key(Platform::Linux, &provenance(), &commands(7000)).unwrap();
    assert_eq!(base.len(), 64);
    assert!(base.bytes().all(|b| b.is_ascii_hexdigit()));
    assert_eq!(
        Some(&base),
        key(Platform::Linux, &provenance(), &commands(7000)).as_ref()
    );
    let mut version = provenance();
    version.app_version = Some("2.0.17".into());
    let mut tools = provenance();
    tools.tools.insert("rustc".into(), "rustc 1.100.0".into());
    let mut revision = provenance();
    revision.source_before = identity('b', false);
    for changed in [
        key(Platform::Web, &provenance(), &commands(7000)),
        key(Platform::Linux, &provenance(), &commands(7001)),
        key(Platform::Linux, &version, &commands(7000)),
        key(Platform::Linux, &tools, &commands(7000)),
        key(Platform::Linux, &revision, &commands(7000)),
    ] {
        assert_ne!(changed.unwrap(), base);
    }
}

#[test]
fn the_same_commit_selected_by_branch_or_pull_request_shares_a_key() {
    let mut selected = provenance();
    selected.remote_selection = Some(crate::app_source::RemoteSelection {
        repository: "https://github.com/MostroP2P/app.git".into(),
        requested: "https://github.com/MostroP2P/app/pull/1".into(),
        git_ref: "refs/pull/1/head".into(),
        revision: "a".repeat(40),
    });
    assert_eq!(
        key(Platform::Linux, &selected, &commands(7000)),
        key(Platform::Linux, &provenance(), &commands(7000))
    );
}

#[test]
fn prebuilt_dirty_or_incomplete_builds_are_never_reused() {
    let mut dirty = provenance();
    dirty.source_before = identity('a', true);
    let mut unversioned = provenance();
    unversioned.app_version = None;
    let mut unknown = provenance();
    unknown.source_before = None;
    for provenance in [BuildProvenance::prebuilt(), dirty, unversioned, unknown] {
        assert_eq!(key(Platform::Linux, &provenance, &commands(7000)), None);
    }
}

#[test]
fn a_kept_bundle_is_found_verified_and_marked_cached() {
    let root = tempfile::tempdir().unwrap();
    let cache = BuildCache::open(&root.path().join("bundles")).unwrap();
    let built = bundle(b"app");
    cache
        .store(&key_for(1), &built, &provenance(), at(0))
        .unwrap();
    let kept = cache.lookup(&key_for(1), at(1)).unwrap().unwrap();
    assert_eq!(&kept.fingerprint, built.fingerprint());
    assert_eq!(
        std::fs::read(kept.root.join("lib/libapp.so")).unwrap(),
        b"app"
    );
    assert_eq!(kept.provenance.origin, ArtifactOrigin::Cached);
    let original = BuildProvenance {
        origin: ArtifactOrigin::Cached,
        ..provenance()
    };
    assert_eq!(kept.provenance, original);
}

#[test]
fn an_unknown_key_is_a_miss() {
    let root = tempfile::tempdir().unwrap();
    let cache = BuildCache::open(root.path()).unwrap();
    assert!(cache.lookup(&key_for(9), at(0)).unwrap().is_none());
}

#[test]
fn a_modified_kept_bundle_is_discarded_and_reported_as_a_miss() {
    use std::os::unix::fs::PermissionsExt;
    let root = tempfile::tempdir().unwrap();
    let cache = BuildCache::open(root.path()).unwrap();
    cache
        .store(&key_for(1), &bundle(b"app"), &provenance(), at(0))
        .unwrap();
    let file = root.path().join(key_for(1)).join("bundle/lib/libapp.so");
    std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o600)).unwrap();
    std::fs::write(&file, b"tampered").unwrap();
    assert!(cache.lookup(&key_for(1), at(1)).unwrap().is_none());
    assert!(!root.path().join(key_for(1)).exists());
}

#[test]
fn an_unreadable_or_mismatched_manifest_is_discarded() {
    let root = tempfile::tempdir().unwrap();
    let cache = BuildCache::open(root.path()).unwrap();
    for manifest in [b"not json".as_slice(), br#"{"format":"other"}"#] {
        cache
            .store(&key_for(1), &bundle(b"app"), &provenance(), at(0))
            .unwrap();
        std::fs::write(root.path().join(key_for(1)).join("manifest.json"), manifest).unwrap();
        assert!(cache.lookup(&key_for(1), at(1)).unwrap().is_none());
        assert!(!root.path().join(key_for(1)).exists());
    }
}

#[test]
fn storing_the_same_key_again_replaces_the_kept_bundle() {
    let root = tempfile::tempdir().unwrap();
    let cache = BuildCache::open(root.path()).unwrap();
    cache
        .store(&key_for(1), &bundle(b"first"), &provenance(), at(0))
        .unwrap();
    let rebuilt = bundle(b"second");
    cache
        .store(&key_for(1), &rebuilt, &provenance(), at(1))
        .unwrap();
    let kept = cache.lookup(&key_for(1), at(2)).unwrap().unwrap();
    assert_eq!(&kept.fingerprint, rebuilt.fingerprint());
}

#[test]
fn the_least_recently_used_bundles_beyond_the_limit_are_evicted() {
    let root = tempfile::tempdir().unwrap();
    let cache = BuildCache::open(root.path()).unwrap();
    let built = bundle(b"app");
    for seed in 0..MAX_KEPT as u8 {
        cache
            .store(&key_for(seed), &built, &provenance(), at(u64::from(seed)))
            .unwrap();
    }
    cache.lookup(&key_for(0), at(10)).unwrap().unwrap();
    cache
        .store(&key_for(9), &built, &provenance(), at(11))
        .unwrap();
    assert!(!root.path().join(key_for(1)).exists(), "oldest use evicted");
    for seed in [0, 2, 3, 9] {
        assert!(cache.lookup(&key_for(seed), at(12)).unwrap().is_some());
    }
}

#[test]
fn leftovers_of_an_interrupted_store_are_removed_by_the_next_store() {
    let root = tempfile::tempdir().unwrap();
    let cache = BuildCache::open(root.path()).unwrap();
    let staging = root.path().join(".staging-killed");
    std::fs::create_dir_all(staging.join("bundle")).unwrap();
    cache
        .store(&key_for(1), &bundle(b"app"), &provenance(), at(0))
        .unwrap();
    assert!(!staging.exists());
}

#[test]
fn keys_that_are_not_a_digest_cannot_name_a_path() {
    let root = tempfile::tempdir().unwrap();
    let cache = BuildCache::open(root.path()).unwrap();
    for key in ["../escape", "", &"A".repeat(64), &"a".repeat(63)] {
        assert!(cache.lookup(key, at(0)).is_err());
        assert!(
            cache
                .store(key, &bundle(b"app"), &provenance(), at(0))
                .is_err()
        );
    }
}

#[test]
fn a_symlinked_cache_directory_is_refused() {
    let root = tempfile::tempdir().unwrap();
    let real = root.path().join("real");
    std::fs::create_dir(&real).unwrap();
    let link = root.path().join("bundles");
    std::os::unix::fs::symlink(&real, &link).unwrap();
    assert!(BuildCache::open(&link).is_err());
}

#[test]
fn an_entry_that_is_not_a_directory_is_refused_not_followed() {
    let root = tempfile::tempdir().unwrap();
    let cache = BuildCache::open(root.path()).unwrap();
    std::fs::write(root.path().join(key_for(1)), b"file").unwrap();
    let outside = tempfile::tempdir().unwrap();
    std::os::unix::fs::symlink(outside.path(), root.path().join(key_for(2))).unwrap();
    assert!(cache.lookup(&key_for(1), at(0)).is_err());
    assert!(cache.lookup(&key_for(2), at(0)).is_err());
    assert!(outside.path().exists());
}

#[test]
fn eviction_drops_unreadable_entries_first_and_ignores_foreign_names() {
    let root = tempfile::tempdir().unwrap();
    let cache = BuildCache::open(root.path()).unwrap();
    let foreign = root.path().join("operator-notes");
    std::fs::create_dir(&foreign).unwrap();
    let broken = root.path().join(key_for(7));
    std::fs::create_dir(&broken).unwrap();
    let built = bundle(b"app");
    for seed in 0..MAX_KEPT as u8 {
        cache
            .store(&key_for(seed), &built, &provenance(), at(u64::from(seed)))
            .unwrap();
    }
    assert!(
        !broken.exists(),
        "an entry without a manifest is evicted first"
    );
    assert!(foreign.exists(), "only kept bundles are evicted");
    for seed in 0..MAX_KEPT as u8 {
        assert!(cache.lookup(&key_for(seed), at(9)).unwrap().is_some());
    }
}
