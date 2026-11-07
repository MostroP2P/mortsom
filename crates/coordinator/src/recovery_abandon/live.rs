//! Real stack inspection and retained-storage removal.
use std::path::{Path, PathBuf};

use async_trait::async_trait;
use mortsom_ui_driver::process::CommandRunner;

use super::RetainedStorage;
use super::assess::{LiveFacts, StackInspector};
use crate::stack::{self, ProjectOwner};

/// Reads descriptors from disk and ownership labels from Docker.
pub struct LiveStacks<'a> {
    /// Runs `docker`.
    pub runner: &'a dyn CommandRunner,
}

#[async_trait]
impl StackInspector for LiveStacks<'_> {
    async fn facts_for(
        &self,
        recorded: &Path,
        selected: &Path,
        original: &crate::stack::StackFingerprint,
    ) -> Result<LiveFacts, String> {
        self.inspect(recorded, selected, Some(original)).await
    }

    async fn facts(&self, recorded: &Path, selected: &Path) -> Result<LiveFacts, String> {
        self.inspect(recorded, selected, None).await
    }
}

impl LiveStacks<'_> {
    async fn inspect(
        &self,
        recorded: &Path,
        selected: &Path,
        original: Option<&crate::stack::StackFingerprint>,
    ) -> Result<LiveFacts, String> {
        // Canonical like the recorded directory, so one physical stack
        // directory reads as one path in the evidence.
        let selected = selected
            .canonicalize()
            .or_else(|_| std::path::absolute(selected))
            .map_err(|e| e.to_string())?;
        let project = stack::project_owner(self.runner)
            .await
            .map_err(|e| e.to_string())?;
        let mut cache = std::collections::BTreeMap::new();
        let mut identity = |dir: &Path| {
            cache
                .entry(dir.to_path_buf())
                .or_insert_with(|| match original {
                    Some(original) => stack::recorded_identity_for(dir, original),
                    None => stack::recorded_identity(dir),
                })
                .clone()
        };
        let project_identity = match &project {
            ProjectOwner::Directory(dir) => Some(identity(dir)),
            _ => None,
        };
        Ok(LiveFacts {
            recorded: (recorded.to_path_buf(), identity(recorded)),
            selected: (selected.clone(), identity(&selected)),
            project,
            project_identity,
        })
    }
}

/// Removes retained actor profiles and the captured bundle once no client
/// process holds a profile.
pub struct RealStorage;

/// What a removal achieved. `removed` is complete up to the failure, if
/// any, so a report never hides what is already gone.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Removal {
    /// Paths removed, in order.
    pub removed: Vec<PathBuf>,
    /// Why removal stopped, when it did.
    pub error: Option<String>,
}

impl RetainedStorage for RealStorage {
    fn remove(&self, root: &Path) -> Removal {
        let actors = root.join("actors");
        for actor in mortsom_common::Actor::ALL {
            let profile = actors.join(actor.as_str());
            if profile.exists()
                && let Err(error) = ensure_inactive(&profile)
            {
                return Removal {
                    removed: Vec::new(),
                    error: Some(format!("{}: {error}", profile.display())),
                };
            }
        }
        let present = [actors, root.join("bundle")]
            .into_iter()
            .filter(|path| path.exists())
            .collect();
        remove_each(
            present,
            |path| {
                std::fs::remove_dir_all(path)
                    .map_err(|e| format!("cannot remove {}: {e}", path.display()))
            },
            || {
                std::fs::File::open(root)
                    .and_then(|dir| dir.sync_all())
                    .map_err(|e| format!("cannot sync {}: {e}", root.display()))
            },
        )
    }
}

/// Proves an actor's retained storage inactive. A marker left on a CLI
/// actor's storage by an invocation a killed coordinator never saw end is
/// cleared first once no mostro-cli process uses its store (spec §11), as
/// `cleanup` does; the CLI's reconciliation refuses storage laid out for
/// another platform, so an app actor's marker still refuses. Whatever the
/// reconciliation answers, a refusal is the storage's own.
fn ensure_inactive(profile: &Path) -> std::io::Result<()> {
    use mortsom_desktop_driver::storage::ensure_stopped;
    let Err(refusal) = ensure_stopped(profile) else {
        return Ok(());
    };
    let reconciled = off_runtime(|| mortsom_cli_driver::runtime::reconcile(profile)).is_ok();
    if reconciled && ensure_stopped(profile).is_ok() {
        return Ok(());
    }
    Err(refusal)
}

/// Runs `scan`, which blocks (the reconciliation scans `/proc`), off the
/// async runtime's worker threads when it is called on one.
fn off_runtime<T>(scan: impl FnOnce() -> T) -> T {
    match tokio::runtime::Handle::try_current() {
        Ok(handle) if handle.runtime_flavor() == tokio::runtime::RuntimeFlavor::MultiThread => {
            tokio::task::block_in_place(scan)
        }
        _ => scan(),
    }
}

/// Removes `paths` in order, stopping at the first failure but keeping what
/// was removed before it; `sync` runs after the last removal.
fn remove_each(
    paths: Vec<PathBuf>,
    mut remove: impl FnMut(&Path) -> Result<(), String>,
    sync: impl FnOnce() -> Result<(), String>,
) -> Removal {
    let mut removed = Vec::new();
    for path in paths {
        if let Err(error) = remove(&path) {
            return Removal {
                removed,
                error: Some(error),
            };
        }
        removed.push(path);
    }
    Removal {
        removed,
        error: sync().err(),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn paths() -> Vec<PathBuf> {
        vec!["/j/actors".into(), "/j/bundle".into()]
    }

    #[test]
    fn a_failure_after_the_first_removal_keeps_what_was_removed() {
        let removal = remove_each(
            paths(),
            |path| {
                if path.ends_with("bundle") {
                    Err("bundle busy".into())
                } else {
                    Ok(())
                }
            },
            || Ok(()),
        );
        assert_eq!(removal.removed, vec![PathBuf::from("/j/actors")]);
        assert_eq!(removal.error.as_deref(), Some("bundle busy"));
    }

    #[test]
    fn a_failure_at_the_final_sync_keeps_every_removal() {
        let removal = remove_each(paths(), |_| Ok(()), || Err("sync failed".into()));
        assert_eq!(removal.removed, paths());
        assert_eq!(removal.error.as_deref(), Some("sync failed"));
    }

    /// A journal directory whose Alice storage, laid out for `platform`,
    /// keeps the marker of a runtime a killed coordinator never saw end.
    fn interrupted(
        platform: mortsom_desktop_driver::Platform,
    ) -> (tempfile::TempDir, PathBuf, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap().join("journal");
        let profile = root.join("actors").join("alice");
        std::fs::create_dir_all(root.join("actors")).unwrap();
        mortsom_desktop_driver::storage::ActorStorage::Fresh(profile.clone())
            .prepare(platform)
            .unwrap();
        mortsom_desktop_driver::storage::ActiveRuntime::begin(&profile).unwrap();
        (dir, root, profile)
    }

    /// An app actor's marker may stand for a client that still runs:
    /// abandonment keeps refusing to remove its storage, as it always did.
    #[test]
    fn an_app_actor_marker_still_refuses_removal() {
        let (_dir, root, profile) = interrupted(mortsom_desktop_driver::Platform::Linux);

        let removal = RealStorage.remove(&root);

        assert!(removal.removed.is_empty());
        let error = removal.error.unwrap();
        assert!(error.contains("unverified prior shutdown"), "{error}");
        assert!(mortsom_desktop_driver::storage::ensure_stopped(&profile).is_err());
    }

    /// A CLI run killed mid-invocation leaves a marker that only the CLI's
    /// reconciliation clears; with the run's stack gone, cleanup never gets
    /// that far, so abandonment reconciles it before it removes the storage.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_cli_actor_marker_no_process_backs_is_cleared_and_removed() {
        let (_dir, root, profile) = interrupted(mortsom_desktop_driver::Platform::Cli);

        let removal = RealStorage.remove(&root);

        if removal.error.is_some()
            && mortsom_cli_driver::runtime::reconcile(&profile)
                .is_err_and(|e| e.to_string().contains("/proc does not list every process"))
        {
            // This host's /proc proves no stop (mounted with hidepid, or the
            // procfs of another PID namespace): the storage stays refused.
            assert!(removal.removed.is_empty());
            assert!(mortsom_desktop_driver::storage::ensure_stopped(&profile).is_err());
            return;
        }
        assert_eq!(removal.error, None);
        assert_eq!(removal.removed, vec![root.join("actors")]);
        assert!(!profile.exists());
    }

    #[test]
    fn a_clean_removal_reports_no_error() {
        let removal = remove_each(paths(), |_| Ok(()), || Ok(()));
        assert_eq!(
            removal,
            Removal {
                removed: paths(),
                error: None
            }
        );
    }
}
