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

impl RetainedStorage for RealStorage {
    fn remove(&self, root: &Path) -> Result<Vec<PathBuf>, String> {
        let actors = root.join("actors");
        for actor in mortsom_common::Actor::ALL {
            let profile = actors.join(actor.as_str());
            if profile.exists() {
                mortsom_desktop_driver::storage::ensure_stopped(&profile)
                    .map_err(|e| format!("{}: {e}", profile.display()))?;
            }
        }
        let mut removed = Vec::new();
        for path in [actors, root.join("bundle")] {
            if path.exists() {
                std::fs::remove_dir_all(&path)
                    .map_err(|e| format!("cannot remove {}: {e}", path.display()))?;
                removed.push(path);
            }
        }
        std::fs::File::open(root)
            .and_then(|dir| dir.sync_all())
            .map_err(|e| e.to_string())?;
        Ok(removed)
    }
}
