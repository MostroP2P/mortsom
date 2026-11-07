//! Read-only checks for the app-owned desktop build pipeline.
use std::path::{Path, PathBuf};

use mortsom_desktop_driver::Platform;

use super::{CheckStatus, DoctorReport, HostProbe};

pub(super) async fn check(
    source: Option<&Path>,
    platform: Platform,
    host: &dyn HostProbe,
    report: &mut DoctorReport,
) {
    if let Some(source) = source {
        report.push(
            "app executable codegen script",
            if host.is_executable(&source.join("scripts/frb-generate.sh")) {
                CheckStatus::Pass
            } else {
                CheckStatus::Fail
            },
            "the app-owned codegen script must be executable",
        );
    }
    for tool in [
        "env",
        "bash",
        "sed",
        "head",
        "grep",
        "flutter_rust_bridge_codegen",
        "cargo-expand",
        "rustc",
    ] {
        check_tool(tool, host, report);
    }
    if let Some(source) = source {
        probe(
            "app codegen pin",
            "env",
            &[
                &format!("--chdir={}", source.display()),
                "bash",
                "./scripts/frb-generate.sh",
                "--check",
            ],
            host,
            report,
        )
        .await;
    }
    match platform {
        Platform::Linux => {
            check_c_toolchain(host, report);
            for tool in ["cmake", "ninja", "pkg-config"] {
                check_tool(tool, host, report);
            }
            probe(
                "Linux development libraries",
                "pkg-config",
                &[
                    "--print-errors",
                    "--exists",
                    "gtk+-3.0",
                    "libsecret-1 >= 0.18.4",
                ],
                host,
                report,
            )
            .await;
        }
        Platform::Web => check_web(source, host, report).await,
        // A CLI build needs no Flutter toolchain; `doctor --profile cli`
        // checks the executable and its discovery instead.
        Platform::Cli => {}
    }
}

/// The C toolchain Flutter's Linux build uses: each role, the names it may
/// have in order of preference, and the package that usually provides it.
const C_TOOLCHAIN: [(&str, &[&str], &str); 3] = [
    ("linker", &["ld.lld", "ld"], "lld"),
    ("compiler", &["clang"], "clang"),
    ("archiver", &["llvm-ar", "ar"], "llvm"),
];

/// Flutter's Linux build resolves `clang++` on `PATH` through its symlinks
/// and takes the linker, compiler and archiver from that directory, or stops
/// with `Failed to find any of [...]` minutes into the build (`flutter_tools`,
/// `native_assets/linux/native_assets.dart`). The check looks exactly there.
fn check_c_toolchain(host: &dyn HostProbe, report: &mut DoctorReport) {
    let (status, detail) = match c_toolchain(host) {
        Ok(dir) => (
            CheckStatus::Pass,
            format!("{} has a linker, a compiler and an archiver", dir.display()),
        ),
        Err(missing) => (CheckStatus::Fail, missing),
    };
    report.push("build C toolchain", status, detail);
}

fn c_toolchain(host: &dyn HostProbe) -> Result<PathBuf, String> {
    let clang = host.which("clang++").ok_or(
        "clang++ not found on PATH; Flutter's Linux build compiles with it (install clang)",
    )?;
    let dir = host
        .resolve(&clang)
        .and_then(|real| real.parent().map(Path::to_path_buf))
        .ok_or_else(|| format!("{} does not resolve to a file", clang.display()))?;
    let missing: Vec<String> = C_TOOLCHAIN
        .iter()
        .filter(|(_, names, _)| !names.iter().any(|name| host.exists(&dir.join(name))))
        .map(|(role, names, package)| {
            format!("no {role} ({}; install {package})", names.join(" or "))
        })
        .collect();
    if missing.is_empty() {
        Ok(dir)
    } else {
        Err(format!(
            "{} {}: Flutter's Linux build takes them from the directory clang++ resolves to",
            dir.display(),
            missing.join(", ")
        ))
    }
}

fn check_tool(tool: &str, host: &dyn HostProbe, report: &mut DoctorReport) {
    let found = host.which(tool).is_some();
    report.push(
        format!("build {tool}"),
        if found {
            CheckStatus::Pass
        } else {
            CheckStatus::Fail
        },
        if found {
            "executable found"
        } else {
            "executable not found"
        },
    );
}

async fn probe(
    name: &str,
    program: &str,
    args: &[&str],
    host: &dyn HostProbe,
    report: &mut DoctorReport,
) -> Option<String> {
    let result = match host.which(program) {
        Some(path) => host.run(&path, args).await,
        None => Err(format!("{program} not found")),
    };
    let (status, detail, value) = match result {
        Ok(output) if output.success => (
            CheckStatus::Pass,
            "prerequisite verified".to_owned(),
            Some(output.stdout),
        ),
        Ok(output) => (CheckStatus::Fail, output.combined(), None),
        Err(error) => (CheckStatus::Fail, error, None),
    };
    report.push(
        name,
        status,
        mortsom_common::Redactor::default().redact(&detail),
    );
    value
}

async fn check_web(source: Option<&Path>, host: &dyn HostProbe, report: &mut DoctorReport) {
    for tool in ["wasm-pack", "rustup", "python3"] {
        check_tool(tool, host, report);
    }
    if let Some(source) = source {
        report.push(
            "app scripts/build-web.sh",
            if host.is_executable(&source.join("scripts/build-web.sh")) {
                CheckStatus::Pass
            } else {
                CheckStatus::Fail
            },
            "the app-owned Web build script must be executable",
        );
    }
    // `rustup run` without --install never installs a missing toolchain.
    let root = probe(
        "Web nightly toolchain",
        "rustup",
        &["run", "nightly", "rustc", "--print", "sysroot"],
        host,
        report,
    )
    .await;
    let sources = root
        .as_deref()
        .map(str::trim)
        .filter(|root| Path::new(root).is_absolute())
        .is_some_and(|root| {
            host.is_file(&Path::new(root).join("lib/rustlib/src/rust/library/std/Cargo.toml"))
        });
    report.push(
        "Web nightly rust-src",
        if sources {
            CheckStatus::Pass
        } else {
            CheckStatus::Fail
        },
        "nightly standard-library sources are required by the app's build-std pipeline",
    );
}

#[cfg(test)]
#[path = "desktop_build_tests.rs"]
mod tests;
