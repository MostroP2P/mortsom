//! Offline settings for inspecting artifacts and cleaning recorded resources.
use super::{Config, ConfigError};
use mortsom_common::Lane;
use serde::Deserialize;
use std::path::Path;

/// Nonsecret run settings used without daemon or wallet connections.
#[derive(Debug, Clone, Deserialize)]
pub struct ArtifactSettings {
    /// Base directory containing per-run evidence.
    #[serde(default = "super::run::default_artifacts_dir")]
    pub artifacts_dir: String,
    /// Explicit lane used by the accounting command.
    pub lane: Lane,
}

/// Configuration projection that cannot launch a trade or make a payment.
#[derive(Debug, Clone, Deserialize)]
pub struct MaintenanceConfig {
    /// Artifact location and lane only; spend caps are not consumed here.
    pub run: ArtifactSettings,
}

impl MaintenanceConfig {
    /// Loads only maintenance fields; runtime env references are irrelevant.
    pub fn load(path: &Path) -> Result<Self, ConfigError> {
        let config: Self = project(path, &["run"])?
            .try_into()
            .map_err(|error: toml::de::Error| ConfigError::Parse(error.to_string()))?;
        config.run.validate()?;
        Ok(config)
    }
}

impl ArtifactSettings {
    /// Reads only artifact location/lane, without resolving runtime credentials.
    pub fn load(path: &Path) -> Result<Self, ConfigError> {
        let raw = project(path, &["run"])?;
        Self::from_run(raw.get("run").cloned(), |name| std::env::var(name).ok())
    }

    pub(super) fn from_run(
        raw: Option<toml::Value>,
        lookup: impl Fn(&str) -> Option<String>,
    ) -> Result<Self, ConfigError> {
        let mut raw = raw.ok_or_else(|| ConfigError::Parse("missing run settings".into()))?;
        if let Some(table) = raw.as_table_mut() {
            table.retain(|name, _| matches!(name, "artifacts_dir" | "lane"));
        }
        let settings: Self = mortsom_common::env_resolve::resolve_env_with(raw, lookup)?
            .try_into()
            .map_err(|error: toml::de::Error| ConfigError::Parse(error.to_string()))?;
        settings.validate()?;
        Ok(settings)
    }

    fn validate(&self) -> Result<(), ConfigError> {
        if self.artifacts_dir.trim().is_empty() || self.artifacts_dir.contains('\0') {
            return Err(ConfigError::Invalid(vec![
                "run.artifacts_dir must name a directory".into(),
            ]));
        }
        Ok(())
    }
}

fn project(path: &Path, sections: &[&str]) -> Result<toml::Value, ConfigError> {
    let mut raw = super::load::read_layered(path)?;
    super::load::reject_literal_secrets(&raw)?;
    let table = raw
        .as_table_mut()
        .ok_or_else(|| ConfigError::Parse("expected configuration table".into()))?;
    table.retain(|name, _| sections.contains(&name));
    if let Some(run) = table.get_mut("run").and_then(toml::Value::as_table_mut) {
        run.retain(|name, _| matches!(name, "artifacts_dir" | "lane"));
    }
    mortsom_common::env_resolve::resolve_env_with(raw, |name| std::env::var(name).ok())
        .map_err(Into::into)
}

impl From<&Config> for MaintenanceConfig {
    fn from(config: &Config) -> Self {
        Self {
            run: ArtifactSettings {
                artifacts_dir: config.run.artifacts_dir.clone(),
                lane: config.run.lane,
            },
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn omitted_artifact_directory_keeps_the_existing_default() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("minimal.toml");
        std::fs::write(&path, "[run]\nlane='local'\n").unwrap();
        assert_eq!(
            ArtifactSettings::load(&path).unwrap().artifacts_dir,
            "./artifacts"
        );
        assert_eq!(
            MaintenanceConfig::load(&path).unwrap().run.artifacts_dir,
            "./artifacts"
        );
    }

    #[test]
    fn desktop_artifact_settings_load_without_any_runtime_environment_references() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("desktop.toml");
        std::fs::write(
            &path,
            include_str!("../../../../config/desktop.example.toml"),
        )
        .unwrap();
        let loaded = MaintenanceConfig::load(&path).unwrap();
        assert_eq!(loaded.run.artifacts_dir, "./artifacts");
        std::fs::write(
            dir.path().join("desktop.local.toml"),
            "[run]\nartifacts_dir='somewhere-else'\n",
        )
        .unwrap();
        assert_eq!(
            MaintenanceConfig::load(&path).unwrap().run.artifacts_dir,
            "somewhere-else"
        );
    }
}
