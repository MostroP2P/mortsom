#![allow(clippy::unwrap_used)]
use super::*;
use crate::doctor::CommandOutput;
use async_trait::async_trait;
use std::{collections::BTreeMap, path::PathBuf, sync::Mutex};

#[derive(Default)]
struct Host {
    calls: Mutex<Vec<(String, Vec<String>)>>,
    failures: BTreeMap<String, String>,
    missing: Option<String>,
    rust_src: bool,
    codegen_executable: bool,
}

#[async_trait]
impl HostProbe for Host {
    fn env(&self, _: &str) -> Option<String> {
        None
    }
    fn which(&self, name: &str) -> Option<PathBuf> {
        (self.missing.as_deref() != Some(name)).then(|| PathBuf::from(name))
    }
    fn exists(&self, path: &Path) -> bool {
        self.is_file(path)
    }
    fn is_file(&self, path: &Path) -> bool {
        self.rust_src || !path.starts_with("/toolchain")
    }
    fn is_executable(&self, path: &Path) -> bool {
        if path.ends_with("scripts/frb-generate.sh") {
            return self.codegen_executable;
        }
        self.is_file(path)
    }
    async fn run(&self, program: &Path, args: &[&str]) -> Result<CommandOutput, String> {
        let name = program.to_string_lossy().into_owned();
        self.calls
            .lock()
            .unwrap()
            .push((name.clone(), args.iter().map(|s| (*s).into()).collect()));
        if let Some(error) = self.failures.get(&name) {
            return Err(error.clone());
        }
        Ok(CommandOutput {
            success: true,
            stdout: if name == "rustup" {
                "/toolchain\n".into()
            } else {
                String::new()
            },
            stderr: String::new(),
        })
    }
}

fn failed(report: &DoctorReport, name: &str) -> bool {
    report
        .checks
        .iter()
        .any(|c| c.name == name && c.status == CheckStatus::Fail)
}

#[tokio::test]
async fn linux_checks_the_pinned_codegen_and_native_libraries_without_building() {
    let host = Host {
        codegen_executable: true,
        ..Host::default()
    };
    let mut report = DoctorReport::default();
    check(Path::new("/app"), Platform::Linux, &host, &mut report).await;
    let calls = host.calls.lock().unwrap();
    assert!(calls.contains(&(
        "env".into(),
        vec![
            "--chdir=/app".into(),
            "bash".into(),
            "./scripts/frb-generate.sh".into(),
            "--check".into()
        ]
    )));
    assert!(calls.contains(&(
        "pkg-config".into(),
        vec![
            "--print-errors".into(),
            "--exists".into(),
            "gtk+-3.0".into(),
            "libsecret-1 >= 0.18.4".into()
        ]
    )));
    assert_eq!(
        calls.len(),
        2,
        "doctor must never invoke a build or generator mutation"
    );
    assert!(report.is_healthy());
}

#[tokio::test]
async fn build_prerequisite_failures_are_reported_with_sanitized_details() {
    let mut host = Host {
        missing: Some("ninja".into()),
        ..Host::default()
    };
    host.failures
        .insert("env".into(), "tool failed secret=private-value".into());
    host.failures
        .insert("pkg-config".into(), "libraries unavailable".into());
    let mut report = DoctorReport::default();
    check(Path::new("/app"), Platform::Linux, &host, &mut report).await;
    assert!(failed(&report, "build ninja"));
    assert!(failed(&report, "app codegen pin"));
    assert!(failed(&report, "Linux development libraries"));
    assert!(failed(&report, "app executable codegen script"));
    assert!(!format!("{report:?}").contains("private-value"));
}

#[tokio::test]
async fn web_requires_nightly_rust_sources_and_never_invokes_wasm_pack_build() {
    for rust_src in [false, true] {
        let host = Host {
            rust_src,
            ..Host::default()
        };
        let mut report = DoctorReport::default();
        check(Path::new("/app"), Platform::Web, &host, &mut report).await;
        assert_eq!(failed(&report, "Web nightly rust-src"), !rust_src);
        let calls = host.calls.lock().unwrap();
        assert!(calls.contains(&(
            "rustup".into(),
            vec![
                "run".into(),
                "nightly".into(),
                "rustc".into(),
                "--print".into(),
                "sysroot".into()
            ]
        )));
        assert_eq!(calls.len(), 2);
        assert!(report.checks.iter().any(|c| c.name == "build wasm-pack"));
    }
}
