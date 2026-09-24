#![allow(clippy::unwrap_used, clippy::expect_used)]
use super::*;
use crate::doctor::{CheckResult, CommandOutput};
use async_trait::async_trait;
use std::{collections::BTreeMap, path::PathBuf, sync::Mutex};

#[derive(Default)]
struct Host {
    calls: Mutex<Vec<(String, Vec<String>)>>,
    failures: BTreeMap<String, String>,
    missing: Option<String>,
    rust_src: bool,
    codegen_executable: bool,
    /// Where `clang++` resolves to, as a distribution's symlinks resolve it.
    resolved: Option<PathBuf>,
    /// Files that do not exist.
    absent: Vec<PathBuf>,
    /// Every path whose existence was asked.
    looked: Mutex<Vec<PathBuf>>,
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
        self.looked.lock().unwrap().push(path.to_path_buf());
        !self.absent.iter().any(|absent| absent == path) && self.is_file(path)
    }
    fn resolve(&self, path: &Path) -> Option<PathBuf> {
        match &self.resolved {
            Some(target) if path == Path::new("clang++") => Some(target.clone()),
            _ => Some(path.to_path_buf()),
        }
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
    check(Some(Path::new("/app")), Platform::Linux, &host, &mut report).await;
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
    check(Some(Path::new("/app")), Platform::Linux, &host, &mut report).await;
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
        check(Some(Path::new("/app")), Platform::Web, &host, &mut report).await;
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

#[tokio::test]
async fn managed_source_checks_tools_without_running_app_scripts_or_fetching() {
    let host = Host {
        rust_src: true,
        ..Host::default()
    };
    for platform in [Platform::Linux, Platform::Web] {
        let mut report = DoctorReport::default();
        check(None, platform, &host, &mut report).await;
        assert!(report.is_healthy(), "{report:?}");
    }
    let calls = host.calls.lock().unwrap();
    assert!(
        calls
            .iter()
            .all(|(name, _)| name == "pkg-config" || name == "rustup")
    );
}

const LLVM: &str = "/usr/lib/llvm-14/bin";

/// `clang++` on `PATH` is `/usr/bin/clang++`, a symlink into LLVM's own
/// directory, as on Ubuntu and Pop!_OS 22.04.
fn llvm_host(absent: &[&str]) -> Host {
    Host {
        resolved: Some(Path::new(LLVM).join("clang")),
        absent: absent
            .iter()
            .map(|name| Path::new(LLVM).join(name))
            .collect(),
        ..Host::default()
    }
}

async fn linux_toolchain(host: &Host) -> CheckResult {
    let mut report = DoctorReport::default();
    check(None, Platform::Linux, host, &mut report).await;
    report
        .checks
        .into_iter()
        .find(|c| c.name == "build C toolchain")
        .expect("the Linux build checks the C toolchain")
}

/// Flutter's Linux build resolves `clang++` on `PATH` through its symlinks
/// and takes the linker, compiler and archiver from that directory
/// (`flutter_tools`, `native_assets/linux/native_assets.dart`). The check
/// looks exactly there, not next to the symlink.
#[tokio::test]
async fn the_c_toolchain_is_looked_for_where_flutter_resolves_clang() {
    let host = llvm_host(&[]);

    let toolchain = linux_toolchain(&host).await;

    assert_eq!(toolchain.status, CheckStatus::Pass, "{toolchain:?}");
    let looked = host.looked.lock().unwrap();
    assert!(
        looked.contains(&Path::new(LLVM).join("ld.lld")),
        "{looked:?}"
    );
    assert!(
        !looked.iter().any(|p| p.starts_with("/usr/bin")),
        "{looked:?}"
    );
}

/// The live failure of 2026-09-23: `clang-14` without `lld-14`, so
/// `flutter build linux` stopped with `Failed to find any of [ld.lld, ld]`
/// minutes into the build while doctor reported healthy.
#[tokio::test]
async fn a_missing_linker_fails_naming_it_the_directory_and_the_package() {
    let toolchain = linux_toolchain(&llvm_host(&["ld.lld", "ld"])).await;

    assert_eq!(toolchain.status, CheckStatus::Fail);
    for expected in ["ld.lld", LLVM, "lld"] {
        assert!(toolchain.detail.contains(expected), "{}", toolchain.detail);
    }
}

/// Flutter takes the first of `ld.lld`, `ld` it finds: `ld` alone will do.
#[tokio::test]
async fn a_plain_ld_next_to_clang_is_a_linker() {
    let toolchain = linux_toolchain(&llvm_host(&["ld.lld"])).await;

    assert_eq!(toolchain.status, CheckStatus::Pass, "{toolchain:?}");
}

#[tokio::test]
async fn a_missing_archiver_fails_naming_it() {
    let toolchain = linux_toolchain(&llvm_host(&["llvm-ar", "ar"])).await;

    assert_eq!(toolchain.status, CheckStatus::Fail);
    assert!(toolchain.detail.contains("llvm-ar"), "{}", toolchain.detail);
}

/// Flutter asks for `clang++`, not `clang`: one without the other fails.
#[tokio::test]
async fn clang_plus_plus_missing_from_path_fails() {
    let host = Host {
        missing: Some("clang++".into()),
        ..llvm_host(&[])
    };

    let toolchain = linux_toolchain(&host).await;

    assert_eq!(toolchain.status, CheckStatus::Fail);
    assert!(toolchain.detail.contains("clang++"), "{}", toolchain.detail);
}

/// Web compiles no native code with this toolchain.
#[tokio::test]
async fn web_does_not_check_the_c_toolchain() {
    let host = Host {
        rust_src: true,
        ..llvm_host(&["ld.lld", "ld"])
    };
    let mut report = DoctorReport::default();

    check(None, Platform::Web, &host, &mut report).await;

    assert!(!report.checks.iter().any(|c| c.name == "build C toolchain"));
}
