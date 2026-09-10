//! Public source/build facts attached to the exact captured client bundle.
use crate::doctor::HostProbe;
use mortsom_desktop_driver::Platform;
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, path::Path};

/// How the captured client artifact was supplied.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArtifactOrigin {
    /// Built by this invocation from the declared app checkout.
    Source,
    /// Supplied by the operator; source/build history is unknown.
    Prebuilt,
}

/// Public Git identity; raw status paths and source contents are never retained.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceIdentity {
    /// Full Git object ID.
    pub revision: String,
    /// Any tracked or untracked local changes reported by Git.
    pub dirty: bool,
}

/// Build facts; prebuilt artifacts explicitly have no claimed build history.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BuildProvenance {
    /// Source build or supplied artifact.
    pub origin: ArtifactOrigin,
    /// Checkout state before generation.
    pub source_before: Option<SourceIdentity>,
    /// Checkout state after the complete build.
    pub source_after: Option<SourceIdentity>,
    /// Rust package version compiled into the app, when built here.
    pub app_version: Option<String>,
    /// Observed compiler/generator versions, without tool output bodies.
    pub tools: BTreeMap<String, String>,
    /// Actual build arguments, with the checkout represented by `<source>`.
    pub commands: Vec<Vec<String>>,
}

impl BuildProvenance {
    /// Honest metadata for a prebuilt artifact with unknown source history.
    pub fn prebuilt() -> Self {
        Self {
            origin: ArtifactOrigin::Prebuilt,
            source_before: None,
            source_after: None,
            app_version: None,
            tools: BTreeMap::new(),
            commands: Vec::new(),
        }
    }

    /// Captures source and selected tool facts before generation can begin.
    pub async fn begin(
        source: &Path,
        platform: Platform,
        host: &dyn HostProbe,
    ) -> Result<Self, String> {
        let identity = source_identity(source, host).await?;
        let version = package_version(source, host).await?;
        let mut tools = BTreeMap::new();
        for tool in ["flutter", "rustc", "cargo", "flutter_rust_bridge_codegen"] {
            tools.insert(
                tool.into(),
                tool_version(source, tool, &["--version"], host).await?,
            );
        }
        let selected: &[&str] = match platform {
            Platform::Linux => &["clang", "cmake", "ninja", "pkg-config"],
            Platform::Web => &["wasm-pack", "python3"],
        };
        for tool in selected {
            tools.insert(
                (*tool).into(),
                tool_version(source, tool, &["--version"], host).await?,
            );
        }
        if platform == Platform::Web {
            tools.insert(
                "rustc-nightly".into(),
                tool_version(
                    source,
                    "rustup",
                    &["run", "nightly", "rustc", "--version"],
                    host,
                )
                .await?,
            );
        }
        Ok(Self {
            origin: ArtifactOrigin::Source,
            source_before: Some(identity),
            source_after: None,
            app_version: Some(version),
            tools,
            commands: Vec::new(),
        })
    }

    /// Refuses a checkout/version change and records the final dirty state.
    pub async fn finish(&mut self, source: &Path, host: &dyn HostProbe) -> Result<(), String> {
        let after = source_identity(source, host).await?;
        if self
            .source_before
            .as_ref()
            .is_none_or(|before| before.revision != after.revision)
            || self.app_version.as_deref() != Some(package_version(source, host).await?.as_str())
        {
            return Err("app source revision or package version changed during the build".into());
        }
        self.source_after = Some(after);
        Ok(())
    }
}

async fn output(
    source: &Path,
    program: &str,
    args: &[&str],
    host: &dyn HostProbe,
) -> Result<String, String> {
    let env = host
        .which("env")
        .ok_or("env is required to query build provenance")?;
    let directory = format!("--chdir={}", source.display());
    let mut contextual = vec![directory.as_str(), program];
    contextual.extend_from_slice(args);
    let output = host
        .run(&env, &contextual)
        .await
        .map_err(|_| format!("cannot query {program} for build provenance"))?;
    if !output.success {
        return Err(format!("{program} failed while querying build provenance"));
    }
    Ok(output.stdout)
}

async fn source_identity(source: &Path, host: &dyn HostProbe) -> Result<SourceIdentity, String> {
    let revision = output(source, "git", &["rev-parse", "--verify", "HEAD"], host).await?;
    let revision = revision.trim();
    if !matches!(revision.len(), 40 | 64) || !revision.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err("Git did not return a full source revision".into());
    }
    let status = output(
        source,
        "git",
        &["status", "--porcelain=v1", "-z", "--untracked-files=normal"],
        host,
    )
    .await?;
    Ok(SourceIdentity {
        revision: revision.to_owned(),
        dirty: !status.is_empty(),
    })
}

async fn package_version(source: &Path, host: &dyn HostProbe) -> Result<String, String> {
    let manifest = source.join("rust/Cargo.toml");
    let manifest = manifest.to_str().ok_or("manifest path must be UTF-8")?;
    let raw = output(
        source,
        "cargo",
        &[
            "metadata",
            "--offline",
            "--no-deps",
            "--format-version=1",
            "--manifest-path",
            manifest,
        ],
        host,
    )
    .await?;
    let metadata: serde_json::Value =
        serde_json::from_str(&raw).map_err(|_| "invalid Cargo metadata for the app build")?;
    let version = metadata
        .get("packages")
        .and_then(serde_json::Value::as_array)
        .and_then(|packages| {
            packages
                .iter()
                .find(|package| package["manifest_path"].as_str() == Some(manifest))
        })
        .and_then(|package| package["version"].as_str())
        .filter(|version| {
            !version.is_empty()
                && version.len() <= 64
                && version
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b".+-".contains(&b))
        })
        .ok_or("missing app package version in Cargo metadata")?;
    Ok(version.to_owned())
}

async fn tool_version(
    source: &Path,
    program: &str,
    args: &[&str],
    host: &dyn HostProbe,
) -> Result<String, String> {
    let text = output(source, program, args, host).await?;
    let version = text
        .lines()
        .find(|line| !line.trim().is_empty())
        .map(str::trim)
        .filter(|line| line.len() <= 512 && !line.chars().any(char::is_control))
        .ok_or_else(|| format!("invalid version output from {program}"))?;
    Ok(mortsom_common::Redactor::default().redact(version))
}

#[cfg(test)]
#[path = "provenance_tests.rs"]
mod tests;
