//! The `[android]` configuration section: the pinned emulator matrix.

use serde::{Deserialize, Serialize};

/// `[android]` section.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AndroidConfig {
    /// SDK system image package, e.g. `system-images;android-35;google_apis;x86_64`.
    pub avd_image: String,
    /// AVD device profile, e.g. `Pixel_6`.
    pub device_profile: String,
    /// Android API level; must match `avd_image`.
    pub api_level: u32,
    /// Maximum time to wait for `sys.boot_completed`.
    #[serde(default = "default_boot_timeout")]
    pub boot_timeout_secs: u64,
    /// Default deadline for a single UI wait.
    #[serde(default = "default_ui_timeout")]
    pub ui_timeout_secs: u64,
    /// Name of the clean snapshot restored between scenarios.
    #[serde(default = "default_snapshot")]
    pub snapshot_name: String,
}

fn default_boot_timeout() -> u64 {
    180
}

fn default_ui_timeout() -> u64 {
    30
}

fn default_snapshot() -> String {
    "mortsom-clean".to_owned()
}

impl AndroidConfig {
    /// Appends every problem with this section to `errors`.
    pub fn validate(&self, errors: &mut Vec<String>) {
        match image_api_level(&self.avd_image) {
            None => errors.push(format!(
                "android.avd_image `{}` must look like system-images;android-<api>;<tag>;<abi>",
                self.avd_image
            )),
            Some(level) if level != self.api_level => errors.push(format!(
                "android.api_level {} does not match avd_image api {}",
                self.api_level, level
            )),
            Some(_) => {}
        }
        if self.device_profile.is_empty() {
            errors.push("android.device_profile must not be empty".to_owned());
        }
        if self.boot_timeout_secs == 0 {
            errors.push("android.boot_timeout_secs must be greater than zero".to_owned());
        }
        if self.ui_timeout_secs == 0 {
            errors.push("android.ui_timeout_secs must be greater than zero".to_owned());
        }
        if self.snapshot_name.is_empty() {
            errors.push("android.snapshot_name must not be empty".to_owned());
        }
    }

    /// Filesystem path of the system image relative to the SDK root.
    pub fn image_relative_path(&self) -> String {
        self.avd_image.replace(';', "/")
    }
}

/// Extracts the API level from a system image package name.
pub fn image_api_level(avd_image: &str) -> Option<u32> {
    let mut parts = avd_image.split(';');
    if parts.next()? != "system-images" {
        return None;
    }
    let level = parts.next()?.strip_prefix("android-")?.parse().ok()?;
    parts.next()?;
    parts.next()?;
    Some(level)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    fn valid() -> AndroidConfig {
        AndroidConfig {
            avd_image: "system-images;android-35;google_apis;x86_64".into(),
            device_profile: "Pixel_6".into(),
            api_level: 35,
            boot_timeout_secs: 180,
            ui_timeout_secs: 30,
            snapshot_name: "mortsom-clean".into(),
        }
    }

    fn errors_of(config: &AndroidConfig) -> Vec<String> {
        let mut errors = Vec::new();
        config.validate(&mut errors);
        errors
    }

    #[test]
    fn valid_section_has_no_errors() {
        assert!(errors_of(&valid()).is_empty());
        assert_eq!(
            valid().image_relative_path(),
            "system-images/android-35/google_apis/x86_64"
        );
    }

    #[test]
    fn defaults_apply() {
        let config: AndroidConfig = toml::from_str(
            "avd_image = \"system-images;android-35;google_apis;x86_64\"\ndevice_profile = \"Pixel_6\"\napi_level = 35",
        )
        .unwrap();

        assert_eq!(config.boot_timeout_secs, 180);
        assert_eq!(config.ui_timeout_secs, 30);
        assert_eq!(config.snapshot_name, "mortsom-clean");
    }

    #[test]
    fn api_level_mismatch_is_rejected() {
        let mut config = valid();
        config.api_level = 34;

        assert!(
            errors_of(&config)
                .iter()
                .any(|e| e.contains("does not match"))
        );
    }

    #[test]
    fn malformed_image_and_zero_timeouts_are_reported() {
        let mut config = valid();
        config.avd_image = "android-35".into();
        config.boot_timeout_secs = 0;
        config.ui_timeout_secs = 0;
        config.device_profile.clear();
        config.snapshot_name.clear();

        assert_eq!(errors_of(&config).len(), 5);
    }

    #[test]
    fn image_api_level_parses_only_full_packages() {
        assert_eq!(
            image_api_level("system-images;android-36;google_apis;arm64-v8a"),
            Some(36)
        );
        assert_eq!(
            image_api_level("system-images;android-36;google_apis"),
            None
        );
        assert_eq!(image_api_level("platforms;android-36"), None);
        assert_eq!(image_api_level("system-images;android-x;a;b"), None);
    }
}
