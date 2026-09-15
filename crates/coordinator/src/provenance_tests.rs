#![allow(clippy::unwrap_used)]
use super::*;
use crate::doctor::CommandOutput;
use std::path::PathBuf;

#[derive(Default)]
struct Host {
    fault: Option<&'static str>,
    require_context: bool,
}
#[async_trait::async_trait]
impl HostProbe for Host {
    fn env(&self, _: &str) -> Option<String> {
        None
    }
    fn which(&self, program: &str) -> Option<PathBuf> {
        Some(program.into())
    }
    fn exists(&self, _: &Path) -> bool {
        true
    }
    fn is_file(&self, _: &Path) -> bool {
        true
    }
    fn is_executable(&self, _: &Path) -> bool {
        true
    }
    async fn run(&self, program: &Path, args: &[&str]) -> Result<CommandOutput, String> {
        if self.require_context
            && (!program.ends_with("env") || args.first() != Some(&"--chdir=/app"))
        {
            return Err("tool query used the wrong checkout context".into());
        }
        let (program, args) = if program.ends_with("env") {
            (Path::new(args[1]), &args[2..])
        } else {
            (program, args)
        };
        if self.fault == Some("tool-failed") && program.ends_with("flutter") {
            return Err("token=private-provenance-value".into());
        }
        let stdout = if program.ends_with("git") {
            if args.contains(&"rev-parse") {
                match self.fault {
                    Some("invalid-revision") => "invalid-revision".into(),
                    Some("changed-revision") => "b".repeat(40),
                    _ => "a".repeat(40),
                }
            } else {
                " M lib/main.dart\n".into()
            }
        } else if args.first() == Some(&"metadata") {
            let version = if self.fault == Some("changed-version") {
                "2.0.1"
            } else {
                "2.0.0"
            };
            let manifest = if self.fault == Some("wrong-manifest") {
                "/other/Cargo.toml"
            } else {
                "/app/rust/Cargo.toml"
            };
            serde_json::json!({"packages":[{"manifest_path":manifest, "version":version}]})
                .to_string()
        } else if self.fault == Some("empty-version") {
            String::new()
        } else {
            format!("{} 1.2.3\nunused output body", program.display())
        };
        Ok(CommandOutput {
            success: true,
            stdout,
            stderr: String::new(),
        })
    }
}

#[tokio::test]
async fn source_build_records_git_dirtiness_and_only_selected_tool_versions() {
    let record = BuildProvenance::begin(Path::new("/app"), Platform::Linux, &Host::default())
        .await
        .unwrap();
    assert_eq!(record.origin, ArtifactOrigin::Source);
    assert_eq!(
        record.source_before.as_ref().unwrap().revision,
        "a".repeat(40)
    );
    assert!(record.source_before.as_ref().unwrap().dirty);
    assert!(record.tools.contains_key("flutter"));
    assert!(!record.tools.contains_key("wasm-pack"));
    assert!(!format!("{record:?}").contains("unused output body"));
}

#[test]
fn prebuilt_artifacts_never_invent_a_source_revision_or_app_version() {
    let record = BuildProvenance::prebuilt();
    assert_eq!(record.origin, ArtifactOrigin::Prebuilt);
    assert!(record.source_before.is_none() && record.source_after.is_none());
    assert!(record.app_version.is_none() && record.tools.is_empty());
}

#[tokio::test]
async fn invalid_or_unavailable_source_facts_fail_without_raw_tool_errors() {
    for fault in [
        "invalid-revision",
        "wrong-manifest",
        "empty-version",
        "tool-failed",
    ] {
        let error = BuildProvenance::begin(
            Path::new("/app"),
            Platform::Linux,
            &Host {
                fault: Some(fault),
                ..Host::default()
            },
        )
        .await
        .unwrap_err();
        assert!(!error.contains("private-provenance-value"));
    }
}

#[tokio::test]
async fn changing_source_revision_or_package_version_rejects_the_completed_build() {
    for fault in ["changed-revision", "changed-version"] {
        let mut record = BuildProvenance::begin(Path::new("/app"), Platform::Web, &Host::default())
            .await
            .unwrap();
        assert!(record.tools.contains_key("wasm-pack"));
        assert!(record.tools.contains_key("rustc-nightly"));
        assert!(!record.tools.contains_key("clang"));
        assert!(
            record
                .finish(
                    Path::new("/app"),
                    &Host {
                        fault: Some(fault),
                        ..Host::default()
                    }
                )
                .await
                .is_err()
        );
        assert!(record.source_after.is_none());
    }
    let mut record = BuildProvenance::begin(Path::new("/app"), Platform::Linux, &Host::default())
        .await
        .unwrap();
    record
        .finish(Path::new("/app"), &Host::default())
        .await
        .unwrap();
    assert_eq!(record.source_before, record.source_after);
}

#[test]
fn source_hashes_remain_public_in_metadata_but_registered_secrets_still_win() {
    let revision = "c".repeat(64);
    let mut record = BuildProvenance::prebuilt();
    record.origin = ArtifactOrigin::Source;
    record.source_before = Some(SourceIdentity {
        revision: revision.clone(),
        dirty: true,
    });
    let redactor = mortsom_common::Redactor::default();
    let value = serde_json::json!({"app_build":record, "stderr":revision});
    let mut redacted = crate::artifacts::redact_tree(value.clone(), &redactor);
    super::super::artifacts::restore_provenance(&mut redacted, Some(&record), &redactor).unwrap();
    assert_eq!(redacted["app_build"]["source_before"]["revision"], revision);
    assert_ne!(redacted["stderr"], revision);
    let redactor = redactor.with_secret(&mortsom_common::Secret::new(&revision));
    let mut redacted = crate::artifacts::redact_tree(value, &redactor);
    super::super::artifacts::restore_provenance(&mut redacted, Some(&record), &redactor).unwrap();
    assert!(!redacted.to_string().contains(&revision));
}

#[tokio::test]
async fn compiler_and_metadata_queries_use_the_same_checkout_as_the_build() {
    let host = Host {
        require_context: true,
        ..Host::default()
    };
    let mut record = BuildProvenance::begin(Path::new("/app"), Platform::Web, &host)
        .await
        .unwrap();
    record.finish(Path::new("/app"), &host).await.unwrap();
}

#[test]
fn remote_selection_round_trips_and_old_recovery_provenance_still_loads() {
    let mut build = BuildProvenance::prebuilt();
    let old = serde_json::to_value(&build).unwrap();
    let mut old = old.as_object().unwrap().clone();
    old.remove("remote_selection");
    assert_eq!(
        serde_json::from_value::<BuildProvenance>(old.into()).unwrap(),
        build
    );
    build.origin = ArtifactOrigin::Source;
    build.remote_selection = Some(crate::app_source::RemoteSelection {
        repository: crate::app_source::REPOSITORY.into(),
        requested: "https://github.com/MostroP2P/app/pull/42".into(),
        git_ref: "refs/pull/42/head".into(),
        revision: "a".repeat(40),
    });
    assert_eq!(
        serde_json::from_slice::<BuildProvenance>(&serde_json::to_vec(&build).unwrap()).unwrap(),
        build
    );
}
