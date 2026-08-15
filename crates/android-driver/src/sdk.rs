//! Android SDK layout: where the tools live.

use std::path::{Path, PathBuf};

use thiserror::Error;

/// Environment variables that may point at the SDK, in priority order.
pub const SDK_ENV_VARS: &[&str] = &["ANDROID_HOME", "ANDROID_SDK_ROOT"];

/// Resolves where AVDs live, following the emulator's own precedence:
/// `ANDROID_AVD_HOME`, then `ANDROID_USER_HOME/avd`, then `$HOME/.android/avd`.
pub fn avd_home<E>(env: E) -> Option<PathBuf>
where
    E: Fn(&str) -> Option<String>,
{
    if let Some(dir) = env("ANDROID_AVD_HOME").filter(|v| !v.is_empty()) {
        return Some(PathBuf::from(dir));
    }
    if let Some(dir) = env("ANDROID_USER_HOME").filter(|v| !v.is_empty()) {
        return Some(PathBuf::from(dir).join("avd"));
    }
    env("HOME")
        .filter(|v| !v.is_empty())
        .map(|home| PathBuf::from(home).join(".android").join("avd"))
}

/// SDK location errors.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum SdkError {
    /// No SDK variable is set.
    #[error("Android SDK not located: set ANDROID_HOME (or ANDROID_SDK_ROOT)")]
    NotConfigured,
    /// The configured directory does not exist.
    #[error("Android SDK directory {0} does not exist")]
    Missing(PathBuf),
    /// A required tool is absent.
    #[error("{tool} not found at {path}")]
    ToolMissing {
        /// Tool name.
        tool: &'static str,
        /// Expected path.
        path: PathBuf,
    },
}

/// A located Android SDK.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AndroidSdk {
    root: PathBuf,
}

impl AndroidSdk {
    /// Locates the SDK from an environment lookup and an existence check.
    pub fn locate<E, X>(env: E, exists: X) -> Result<Self, SdkError>
    where
        E: Fn(&str) -> Option<String>,
        X: Fn(&Path) -> bool,
    {
        let root = SDK_ENV_VARS
            .iter()
            .find_map(|var| env(var).filter(|v| !v.is_empty()))
            .map(PathBuf::from)
            .ok_or(SdkError::NotConfigured)?;
        if !exists(&root) {
            return Err(SdkError::Missing(root));
        }
        Ok(Self { root })
    }

    /// Uses an explicit root (tests, overrides).
    pub fn at(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// SDK root directory.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// `emulator` binary.
    pub fn emulator(&self) -> PathBuf {
        self.root.join("emulator").join("emulator")
    }

    /// `adb` binary.
    pub fn adb(&self) -> PathBuf {
        self.root.join("platform-tools").join("adb")
    }

    /// `avdmanager` script.
    pub fn avdmanager(&self) -> PathBuf {
        self.root
            .join("cmdline-tools")
            .join("latest")
            .join("bin")
            .join("avdmanager")
    }

    /// `sdkmanager` script.
    pub fn sdkmanager(&self) -> PathBuf {
        self.root
            .join("cmdline-tools")
            .join("latest")
            .join("bin")
            .join("sdkmanager")
    }

    /// Directory of an installed system image package such as
    /// `system-images;android-35;google_apis;x86_64`.
    pub fn system_image_dir(&self, package: &str) -> PathBuf {
        self.root.join(package.replace(';', "/"))
    }

    /// Verifies that every tool Mortsom needs exists.
    pub fn verify_tools<X: Fn(&Path) -> bool>(&self, exists: X) -> Result<(), SdkError> {
        for (tool, path) in [
            ("emulator", self.emulator()),
            ("adb", self.adb()),
            ("avdmanager", self.avdmanager()),
        ] {
            if !exists(&path) {
                return Err(SdkError::ToolMissing { tool, path });
            }
        }
        Ok(())
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn locate_prefers_android_home_then_sdk_root() {
        let env = |name: &str| match name {
            "ANDROID_SDK_ROOT" => Some("/sdk-root".to_owned()),
            _ => None,
        };
        let sdk = AndroidSdk::locate(env, |_| true).unwrap();
        assert_eq!(sdk.root(), Path::new("/sdk-root"));

        let both = |name: &str| Some(format!("/{name}"));
        assert_eq!(
            AndroidSdk::locate(both, |_| true).unwrap().root(),
            Path::new("/ANDROID_HOME")
        );
    }

    #[test]
    fn avd_home_follows_emulator_precedence() {
        let all = |name: &str| Some(format!("/{name}"));
        assert_eq!(avd_home(all), Some(PathBuf::from("/ANDROID_AVD_HOME")));
        let user = |name: &str| (name != "ANDROID_AVD_HOME").then(|| format!("/{name}"));
        assert_eq!(
            avd_home(user),
            Some(PathBuf::from("/ANDROID_USER_HOME/avd"))
        );
        let home = |name: &str| (name == "HOME").then(|| "/home/me".to_owned());
        assert_eq!(avd_home(home), Some(PathBuf::from("/home/me/.android/avd")));
        assert_eq!(avd_home(|_| None), None);
    }

    #[test]
    fn locate_fails_closed() {
        assert_eq!(
            AndroidSdk::locate(|_| None, |_| true).unwrap_err(),
            SdkError::NotConfigured
        );
        assert_eq!(
            AndroidSdk::locate(|_| Some("/nope".into()), |_| false).unwrap_err(),
            SdkError::Missing(PathBuf::from("/nope"))
        );
    }

    #[test]
    fn tool_paths_follow_sdk_layout() {
        let sdk = AndroidSdk::at("/sdk");
        assert_eq!(sdk.emulator(), PathBuf::from("/sdk/emulator/emulator"));
        assert_eq!(sdk.adb(), PathBuf::from("/sdk/platform-tools/adb"));
        assert_eq!(
            sdk.avdmanager(),
            PathBuf::from("/sdk/cmdline-tools/latest/bin/avdmanager")
        );
        assert_eq!(
            sdk.sdkmanager(),
            PathBuf::from("/sdk/cmdline-tools/latest/bin/sdkmanager")
        );
        assert_eq!(
            sdk.system_image_dir("system-images;android-35;google_apis;x86_64"),
            PathBuf::from("/sdk/system-images/android-35/google_apis/x86_64")
        );
    }

    #[test]
    fn verify_tools_names_the_missing_one() {
        let sdk = AndroidSdk::at("/sdk");
        assert!(sdk.verify_tools(|_| true).is_ok());
        let err = sdk.verify_tools(|p| !p.ends_with("adb")).unwrap_err();
        assert!(matches!(err, SdkError::ToolMissing { tool: "adb", .. }));
    }
}
