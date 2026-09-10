//! Offline publication retains every configured credential redaction value.
use super::{ArtifactSettings, Config, ConfigError};
use mortsom_common::Secret;
use serde::Deserialize;
use std::path::Path;

/// The only configuration values needed to sanitize existing run evidence.
#[derive(Debug, Clone)]
pub struct PublishSettings {
    /// Artifact directory and explicit lane.
    pub artifacts: ArtifactSettings,
    /// NWC URIs or LND credential paths, matching the run's known-secret guard.
    pub secrets: Vec<Secret>,
}

impl PublishSettings {
    /// Resolves publication settings without Docker or daemon/wallet connections.
    pub fn load_with(
        path: &Path,
        stack_dir: &Path,
        lookup: impl Fn(&str) -> Option<String>,
    ) -> Result<Self, ConfigError> {
        Self::load_inner(path, stack_dir, None, lookup)
    }

    /// Resolves stack credential references only when the run recorded a platform
    /// (or none, for a run that never started) and the config references them.
    pub fn load_for_run(
        path: &Path,
        stack_dir: &Path,
        run_id: &mortsom_common::RunId,
        lookup: impl Fn(&str) -> Option<String>,
    ) -> Result<Self, ConfigError> {
        Self::load_inner(path, stack_dir, Some(run_id), lookup)
    }

    fn load_inner(
        path: &Path,
        stack_dir: &Path,
        run_id: Option<&mortsom_common::RunId>,
        lookup: impl Fn(&str) -> Option<String>,
    ) -> Result<Self, ConfigError> {
        let raw = super::load::read_layered(path)?;
        super::load::reject_literal_secrets(&raw)?;
        let artifacts = ArtifactSettings::from_run(raw.get("run").cloned(), &lookup)?;
        let references = references(&raw)?;
        if let Some(run_id) = run_id {
            crate::artifacts::recorded_platform(
                &Path::new(&artifacts.artifacts_dir).join(run_id.as_str()),
            )
            .map_err(|_| {
                ConfigError::Invalid(vec![
                    "cannot verify the recorded publication platform".into(),
                ])
            })?;
        }
        let needs_stack_dir = references
            .iter()
            .any(|name| crate::stack::credential_reference(Path::new("/"), name).is_some());
        let root = if needs_stack_dir {
            Some(
                stack_dir
                    .canonicalize()
                    .map_err(|source| ConfigError::Read {
                        path: stack_dir.display().to_string(),
                        source,
                    })?,
            )
        } else {
            None
        };
        let mut secrets = Vec::new();
        for name in references {
            let value = root
                .as_deref()
                .and_then(|root| crate::stack::credential_reference(root, &name))
                .map(|path| path.display().to_string())
                .or_else(|| lookup(&name))
                .filter(|value| !value.is_empty())
                .ok_or_else(|| ConfigError::MissingEnv(mortsom_common::MissingEnv(vec![name])))?;
            secrets.push(Secret::new(value));
        }
        Ok(Self { artifacts, secrets })
    }
}

#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
enum References {
    Nwc { uri: String },
    Lnd { cert: String, macaroon: String },
}

fn references(raw: &toml::Value) -> Result<Vec<String>, ConfigError> {
    let mut references = Vec::new();
    for actor in ["alice", "bob"] {
        let value = raw
            .get("wallets")
            .and_then(|wallets| wallets.get(actor))
            .cloned()
            .ok_or_else(|| {
                ConfigError::Parse(format!("missing wallet credential references for {actor}"))
            })?;
        let parsed: References = value.try_into().map_err(|_| {
            ConfigError::Parse(format!("invalid wallet credential references for {actor}"))
        })?;
        let values = match parsed {
            References::Nwc { uri } => vec![uri],
            References::Lnd { cert, macaroon } => vec![cert, macaroon],
        };
        for value in values {
            let name = value
                .strip_prefix("env:")
                .filter(|name| {
                    !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
                })
                .ok_or_else(|| {
                    ConfigError::Parse("invalid credential environment reference".into())
                })?;
            references.push(name.to_owned());
        }
    }
    Ok(references)
}

impl From<&Config> for PublishSettings {
    fn from(config: &Config) -> Self {
        Self {
            artifacts: ArtifactSettings {
                artifacts_dir: config.run.artifacts_dir.clone(),
                lane: config.run.lane,
            },
            secrets: config.wallets.secrets().into_iter().cloned().collect(),
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn unreadable_run_metadata_refuses_publication_instead_of_guessing() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("desktop.toml");
        let text = include_str!("../../../../config/desktop.example.toml").replace(
            "artifacts_dir = \"./artifacts\"",
            "artifacts_dir = \"env:REPORTS\"",
        );
        std::fs::write(&path, text).unwrap();
        let run = root.path().join("broken-run");
        std::fs::create_dir(&run).unwrap();
        std::fs::write(run.join("environment.json"), "not json").unwrap();
        let error = PublishSettings::load_for_run(
            &path,
            Path::new("/missing/stack"),
            &"broken-run".parse().unwrap(),
            |name| {
                Some(if name == "REPORTS" {
                    root.path().display().to_string()
                } else {
                    format!("explicit-{name}")
                })
            },
        )
        .unwrap_err();
        assert!(
            error.to_string().contains("recorded publication platform"),
            "{error}"
        );
    }

    #[test]
    fn nwc_publication_keeps_known_values_without_needing_a_stack_directory() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("publish.toml");
        std::fs::write(&path, "[run]\nartifacts_dir='env:REPORTS'\nlane='local'\n[wallets.alice]\nkind='nwc'\nuri='env:ALICE_URI'\n[wallets.bob]\nkind='nwc'\nuri='env:BOB_URI'\n").unwrap();
        let settings = PublishSettings::load_with(&path, Path::new("/missing/stack"), |name| {
            Some(
                match name {
                    "REPORTS" => "reports",
                    "ALICE_URI" => "known-alice-credential",
                    _ => "known-bob-credential",
                }
                .into(),
            )
        })
        .unwrap();
        assert_eq!(settings.artifacts.artifacts_dir, "reports");
        let redactor = mortsom_common::Redactor::new().with_secrets(&settings.secrets);
        assert!(
            !redactor
                .redact("known-alice-credential known-bob-credential")
                .contains("known-")
        );
        assert!(PublishSettings::load_with(&path, Path::new("/missing/stack"), |_| None).is_err());
    }

    #[test]
    fn desktop_publication_resolves_only_credential_paths_without_running_the_stack() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("desktop.toml");
        std::fs::write(
            &path,
            include_str!("../../../../config/desktop.example.toml"),
        )
        .unwrap();
        let settings =
            PublishSettings::load_with(&path, root.path(), |_| Some("ignored-override".into()))
                .unwrap();
        assert_eq!(settings.secrets.len(), 4);
        assert!(settings.secrets.iter().any(|secret| {
            secret.expose()
                == root
                    .path()
                    .join("local/creds/alice/admin.macaroon")
                    .to_str()
                    .unwrap()
        }));
        assert!(
            !root.path().join("ci").exists(),
            "offline publication must not create the stack or credentials"
        );
        std::fs::write(
            root.path().join("desktop.local.toml"),
            "[wallets.alice]\nmacaroon='env:CUSTOM_MACAROON'\n",
        )
        .unwrap();
        assert!(PublishSettings::load_with(&path, root.path(), |_| None).is_err());
    }
}
