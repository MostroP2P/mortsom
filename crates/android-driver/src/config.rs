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
    /// Prefix of the per-actor AVD names (`<prefix>-alice`, `<prefix>-bob`).
    #[serde(default = "default_avd_prefix")]
    pub avd_name_prefix: String,
    /// Console port of the first emulator; each actor uses `base + 2 * index`
    /// (adb uses the next odd port), so Alice and Bob never share a port.
    #[serde(default = "default_console_port")]
    pub base_console_port: u16,
    /// Emulator RAM in megabytes.
    #[serde(default = "default_ram_mb")]
    pub emulator_ram_mb: u32,
    /// Run emulators without a window.
    #[serde(default = "default_true")]
    pub headless: bool,
    /// Appium server port (loopback).
    #[serde(default = "default_appium_port")]
    pub appium_port: u16,
    /// First UiAutomator2 `systemPort`; each actor uses `base + index`.
    #[serde(default = "default_system_port")]
    pub base_system_port: u16,
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

fn default_avd_prefix() -> String {
    "mortsom".to_owned()
}

fn default_console_port() -> u16 {
    5554
}

fn default_ram_mb() -> u32 {
    3072
}

fn default_true() -> bool {
    true
}

fn default_appium_port() -> u16 {
    4723
}

fn default_system_port() -> u16 {
    8200
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
        if self.avd_name_prefix.is_empty()
            || !self
                .avd_name_prefix
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
        {
            errors.push("android.avd_name_prefix must be a non-empty AVD-safe name".to_owned());
        }
        // Emulator console ports are even numbers in 5554..=5682.
        if self.appium_port == 0 || self.base_system_port == 0 {
            errors.push(
                "android.appium_port and android.base_system_port must be non-zero".to_owned(),
            );
        }
        // Every actor takes one system port; the last one must still fit.
        let slots = u16::try_from(crate::avd::Actor::ALL.len()).unwrap_or(u16::MAX);
        if self.base_system_port != 0 && self.base_system_port.checked_add(slots).is_none() {
            errors.push(format!(
                "android.base_system_port must leave room for {slots} actor ports below {}",
                u16::MAX
            ));
        }
        if self.emulator_ram_mb < 1024 {
            errors.push("android.emulator_ram_mb must be at least 1024".to_owned());
        }
        if self.base_console_port < 5554
            || self.base_console_port > 5680
            || !self.base_console_port.is_multiple_of(2)
        {
            errors.push(
                "android.base_console_port must be an even port between 5554 and 5680".to_owned(),
            );
        }
    }

    /// Console port for an actor slot; adb listens on the next port.
    pub fn console_port(&self, slot: u16) -> u16 {
        self.base_console_port + 2 * slot
    }

    /// adb port for an actor slot.
    pub fn adb_port(&self, slot: u16) -> u16 {
        self.console_port(slot) + 1
    }

    /// UiAutomator2 system port for an actor slot.
    pub fn system_port(&self, slot: u16) -> u16 {
        self.base_system_port + slot
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
            avd_name_prefix: "mortsom".into(),
            base_console_port: 5554,
            emulator_ram_mb: 3072,
            headless: true,
            appium_port: 4723,
            base_system_port: 8200,
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
        assert_eq!(config.avd_name_prefix, "mortsom");
        assert_eq!(config.console_port(0), 5554);
        assert_eq!(config.console_port(1), 5556);
        assert_eq!(config.adb_port(1), 5557);
        assert_eq!(config.emulator_ram_mb, 3072);
        assert_eq!(config.system_port(1), 8201);
        assert!(config.headless);
    }

    #[test]
    fn bad_prefix_and_odd_port_are_rejected() {
        let mut config = valid();
        config.avd_name_prefix = "bad name".into();
        config.base_console_port = 5555;

        assert_eq!(errors_of(&config).len(), 2);
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
    #[test]
    fn a_base_system_port_without_room_for_every_actor_is_rejected() {
        let mut config = valid();
        config.base_system_port = u16::MAX;
        assert!(
            errors_of(&config)
                .iter()
                .any(|e| e.contains("base_system_port")),
            "the last actor port would overflow"
        );

        config.base_system_port = u16::MAX - u16::try_from(crate::avd::Actor::ALL.len()).unwrap();
        assert!(errors_of(&config).is_empty(), "{:?}", errors_of(&config));
    }
}
