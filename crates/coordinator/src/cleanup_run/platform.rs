//! Cleanup routing uses recorded run metadata before legacy configuration.
#[cfg(test)]
use super::files;
use super::{DeviceCleanupStatus, Path};
use crate::artifacts::recorded_platform;
use mortsom_desktop_driver::Platform;

pub(super) fn resolve(run_dir: &Path, android_configured: bool) -> DeviceCleanupStatus {
    match recorded_platform(run_dir) {
        Ok(Some(Platform::Linux | Platform::Web)) => DeviceCleanupStatus::DesktopUnsupported,
        Ok(Some(Platform::Android)) if android_configured => DeviceCleanupStatus::AndroidChecked,
        Ok(None) if android_configured => DeviceCleanupStatus::AndroidChecked,
        Ok(None) => DeviceCleanupStatus::DesktopUnsupported,
        Err(error) => {
            tracing::warn!(reason = ?error, "cleanup cannot verify the recorded client platform");
            DeviceCleanupStatus::Unknown
        }
        Ok(Some(Platform::Android)) => DeviceCleanupStatus::Unknown,
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn invalid_or_conflicting_metadata_never_falls_back_to_android() {
        let dir = tempfile::tempdir().unwrap();
        for content in [r#"{"platform":"unknown"}"#, "invalid json"] {
            std::fs::write(dir.path().join(files::SUMMARY), content).unwrap();
            assert_eq!(resolve(dir.path(), true), DeviceCleanupStatus::Unknown);
        }
        std::fs::write(dir.path().join(files::SUMMARY), r#"{"platform":"android"}"#).unwrap();
        std::fs::write(dir.path().join(files::ENVIRONMENT), r#"{"platform":"web"}"#).unwrap();
        assert_eq!(resolve(dir.path(), true), DeviceCleanupStatus::Unknown);
    }
}
