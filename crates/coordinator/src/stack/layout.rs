//! Where the stack keeps its committed inputs and its generated state.
use std::path::{Path, PathBuf};

use super::StackError;

/// Compose project name; also what the ownership labels are matched on.
pub const PROJECT: &str = "mortsom-regtest";

/// Directory the operator selects with `--stack-dir`; every path below is
/// derived from it so nothing else is configurable.
#[derive(Debug, Clone)]
pub struct Layout {
    root: PathBuf,
}

impl Layout {
    /// Canonicalises `dir`; the compose file must already exist there.
    pub fn new(dir: &Path) -> Result<Self, StackError> {
        let root = dir.canonicalize().map_err(|error| {
            StackError::Configuration(format!("stack directory {}: {error}", dir.display()))
        })?;
        let layout = Self { root };
        if !layout.compose_file().is_file() {
            return Err(StackError::Configuration(format!(
                "{} holds no compose.yml; pass --stack-dir pointing at the repository's stack directory",
                layout.root.display()
            )));
        }
        Ok(layout)
    }

    /// The canonical stack directory.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The committed Compose definition.
    pub fn compose_file(&self) -> PathBuf {
        self.root.join("compose.yml")
    }

    /// Daemon settings template with the identity placeholder.
    pub fn template(&self) -> PathBuf {
        self.root.join("mostro-settings.tpl.toml")
    }

    /// The `[anti_abuse_bond]` fragment `up --bond` appends to the settings.
    pub fn bond_template(&self) -> PathBuf {
        self.root.join("mostro-bond.tpl.toml")
    }

    /// Committed declaration of the daemon limits.
    pub fn limits_file(&self) -> PathBuf {
        self.root.join("limits.toml")
    }

    /// Everything `stack up` generates; gitignored and removed by `down`.
    pub fn local_dir(&self) -> PathBuf {
        self.root.join("local")
    }

    /// Bind-mounted into the daemon as `/config`.
    pub fn config_dir(&self) -> PathBuf {
        self.local_dir().join("config")
    }

    /// Carol's credentials, read by the daemon only.
    pub fn daemon_lnd_dir(&self) -> PathBuf {
        self.config_dir().join("lnd")
    }

    /// Rendered daemon settings; owner-only, carries the daemon key.
    pub fn settings_file(&self) -> PathBuf {
        self.config_dir().join("settings.toml")
    }

    /// Actor credentials the harness reads; one directory per actor.
    pub fn creds_dir(&self) -> PathBuf {
        self.local_dir().join("creds")
    }

    /// Public facts written at the end of `up`; no secret is ever in it.
    pub fn descriptor_file(&self) -> PathBuf {
        self.local_dir().join("public.json")
    }
}
