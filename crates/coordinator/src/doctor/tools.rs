//! Host tooling checks: Docker, Android SDK, emulator, adb, avdmanager,
//! system image, Appium + UiAutomator2, Java, Flutter and the app checkout.

use std::path::{Path, PathBuf};

use mortsom_android_driver::AndroidConfig;
use mortsom_app_contract::MobileConfig;

use super::host::HostProbe;
use mortsom_common::check::{CheckStatus, DoctorReport};

/// Environment variables that may point at the Android SDK, in priority order.
pub const SDK_ENV_VARS: &[&str] = &["ANDROID_HOME", "ANDROID_SDK_ROOT"];

/// Runs every host tooling check and appends results to `report`.
pub async fn check_tools(
    host: &dyn HostProbe,
    android: Option<&AndroidConfig>,
    mobile: Option<&MobileConfig>,
    report: &mut DoctorReport,
) {
    check_docker(host, report).await;
    let sdk = check_android_sdk(host, report);
    check_sdk_tool(
        host,
        sdk.as_deref(),
        "emulator",
        "emulator/emulator",
        report,
    )
    .await;
    check_sdk_tool(host, sdk.as_deref(), "adb", "platform-tools/adb", report).await;
    check_sdk_tool(
        host,
        sdk.as_deref(),
        "avdmanager",
        "cmdline-tools/latest/bin/avdmanager",
        report,
    )
    .await;
    check_system_image(host, sdk.as_deref(), android, report);
    check_appium(host, report).await;
    check_java(host, report).await;
    check_flutter(host, mobile, report).await;
    check_mobile_checkout(host, mobile, report);
}

async fn check_docker(host: &dyn HostProbe, report: &mut DoctorReport) {
    let Some(docker) = host.which("docker") else {
        report.push("docker", CheckStatus::Fail, "docker not found on PATH");
        return;
    };
    match host
        .run(&docker, &["info", "--format", "{{.ServerVersion}}"])
        .await
    {
        Ok(out) if out.success => report.push(
            "docker",
            CheckStatus::Pass,
            format!("engine {}", out.stdout.trim()),
        ),
        Ok(out) => report.push(
            "docker",
            CheckStatus::Fail,
            format!("daemon not reachable: {}", first_line(&out.stderr)),
        ),
        Err(e) => report.push("docker", CheckStatus::Fail, e),
    }
}

fn check_android_sdk(host: &dyn HostProbe, report: &mut DoctorReport) -> Option<PathBuf> {
    for var in SDK_ENV_VARS {
        if let Some(value) = host.env(var) {
            let path = PathBuf::from(&value);
            if host.exists(&path) {
                report.push("android-sdk", CheckStatus::Pass, format!("{var}={value}"));
                return Some(path);
            }
            report.push(
                "android-sdk",
                CheckStatus::Fail,
                format!("{var}={value} does not exist"),
            );
            return None;
        }
    }
    report.push(
        "android-sdk",
        CheckStatus::Fail,
        "set ANDROID_HOME (or ANDROID_SDK_ROOT) to the SDK directory",
    );
    None
}

async fn check_sdk_tool(
    host: &dyn HostProbe,
    sdk: Option<&Path>,
    name: &str,
    relative: &str,
    report: &mut DoctorReport,
) {
    let candidate = sdk
        .map(|root| root.join(relative))
        .filter(|path| host.exists(path))
        .or_else(|| host.which(name));
    match candidate {
        Some(path) => report.push(name, CheckStatus::Pass, path.display().to_string()),
        None => report.push(
            name,
            CheckStatus::Fail,
            format!("{name} not found under the SDK ({relative}) or on PATH"),
        ),
    }
}

fn check_system_image(
    host: &dyn HostProbe,
    sdk: Option<&Path>,
    android: Option<&AndroidConfig>,
    report: &mut DoctorReport,
) {
    let Some(android) = android else {
        report.push(
            "system-image",
            CheckStatus::Skip,
            "no [android] configuration",
        );
        return;
    };
    let Some(sdk) = sdk else {
        report.push("system-image", CheckStatus::Skip, "SDK not located");
        return;
    };
    let path = sdk.join(android.image_relative_path());
    if host.exists(&path) {
        report.push("system-image", CheckStatus::Pass, android.avd_image.clone());
    } else {
        report.push(
            "system-image",
            CheckStatus::Fail,
            format!(
                "missing; install with: sdkmanager \"{}\"",
                android.avd_image
            ),
        );
    }
}

async fn check_appium(host: &dyn HostProbe, report: &mut DoctorReport) {
    let Some(appium) = host.which("appium") else {
        report.push(
            "appium",
            CheckStatus::Fail,
            "appium not found on PATH (Appium 2 with the uiautomator2 driver is required)",
        );
        report.push("uiautomator2", CheckStatus::Skip, "appium not found");
        return;
    };
    match host.run(&appium, &["--version"]).await {
        Ok(out) if out.success => report.push(
            "appium",
            CheckStatus::Pass,
            format!("version {}", out.stdout.trim()),
        ),
        Ok(out) => report.push("appium", CheckStatus::Fail, first_line(&out.combined())),
        Err(e) => report.push("appium", CheckStatus::Fail, e),
    }
    match host
        .run(&appium, &["driver", "list", "--installed", "--json"])
        .await
    {
        Ok(out) if out.success && out.stdout.contains("\"uiautomator2\"") => {
            report.push("uiautomator2", CheckStatus::Pass, "driver installed")
        }
        Ok(_) => report.push(
            "uiautomator2",
            CheckStatus::Fail,
            "driver missing; install with: appium driver install uiautomator2",
        ),
        Err(e) => report.push("uiautomator2", CheckStatus::Fail, e),
    }
}

async fn check_java(host: &dyn HostProbe, report: &mut DoctorReport) {
    let Some(java) = host.which("java") else {
        report.push(
            "java",
            CheckStatus::Fail,
            "java not found on PATH (JDK 17 recommended)",
        );
        return;
    };
    match host.run(&java, &["-version"]).await {
        Ok(out) if out.success => {
            report.push("java", CheckStatus::Pass, first_line(&out.combined()))
        }
        Ok(out) => report.push("java", CheckStatus::Fail, first_line(&out.combined())),
        Err(e) => report.push("java", CheckStatus::Fail, e),
    }
}

async fn check_flutter(
    host: &dyn HostProbe,
    mobile: Option<&MobileConfig>,
    report: &mut DoctorReport,
) {
    if mobile.is_some_and(|m| !m.apk_path.is_empty()) {
        report.push("flutter", CheckStatus::Skip, "prebuilt APK configured");
        return;
    }
    let Some(flutter) = host.which("flutter") else {
        report.push("flutter", CheckStatus::Fail, "flutter not found on PATH");
        return;
    };
    match host.run(&flutter, &["--version"]).await {
        Ok(out) if out.success => {
            report.push("flutter", CheckStatus::Pass, first_line(&out.stdout))
        }
        Ok(out) => report.push("flutter", CheckStatus::Fail, first_line(&out.combined())),
        Err(e) => report.push("flutter", CheckStatus::Fail, e),
    }
}

fn check_mobile_checkout(
    host: &dyn HostProbe,
    mobile: Option<&MobileConfig>,
    report: &mut DoctorReport,
) {
    let Some(mobile) = mobile else {
        report.push("mobile-app", CheckStatus::Skip, "no [mobile] configuration");
        return;
    };
    if !mobile.apk_path.is_empty() {
        let apk = Path::new(&mobile.apk_path);
        if host.exists(apk) {
            report.push(
                "mobile-app",
                CheckStatus::Pass,
                format!("apk {}", mobile.apk_path),
            );
        } else {
            report.push(
                "mobile-app",
                CheckStatus::Fail,
                format!("apk {} does not exist", mobile.apk_path),
            );
        }
        return;
    }
    let pubspec = Path::new(&mobile.source_dir).join("pubspec.yaml");
    if host.exists(&pubspec) {
        report.push(
            "mobile-app",
            CheckStatus::Pass,
            format!(
                "checkout {} (profile {})",
                mobile.source_dir, mobile.profile
            ),
        );
    } else {
        report.push(
            "mobile-app",
            CheckStatus::Fail,
            format!("{} is not a Flutter checkout", mobile.source_dir),
        );
    }
}

fn first_line(text: &str) -> String {
    text.lines().next().unwrap_or("").trim().to_owned()
}
