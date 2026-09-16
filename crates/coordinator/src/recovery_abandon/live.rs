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
    async fn facts(&self, recorded: &Path, selected: &Path) -> Result<LiveFacts, String> {
        // Canonical like the recorded directory, so one physical stack
        // directory reads as one path in the evidence.
        let selected = selected
            .canonicalize()
            .or_else(|_| std::path::absolute(selected))
            .map_err(|e| e.to_string())?;
        let project = stack::project_owner(self.runner)
            .await
            .map_err(|e| e.to_string())?;
        let project_identity = match &project {
            ProjectOwner::Directory(dir) => Some(stack::recorded_identity(dir)),
            _ => None,
        };
        Ok(LiveFacts {
            recorded: (recorded.to_path_buf(), stack::recorded_identity(recorded)),
            selected: (selected.clone(), stack::recorded_identity(&selected)),
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
                && let Err(error) = mortsom_desktop_driver::storage::ensure_stopped(&profile)
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
