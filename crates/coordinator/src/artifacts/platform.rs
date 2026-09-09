//! Reads the platform recorded before client startup and in the final report.
use super::{Path, files};
use mortsom_desktop_driver::Platform;
use serde::Deserialize;

#[derive(Deserialize)]
struct Metadata {
    #[serde(default)]
    platform: Option<Platform>,
}

#[derive(Debug)]
pub(crate) enum MetadataError {
    Unreadable,
    Invalid,
    Conflicting,
}

pub(crate) fn recorded_platform(run_dir: &Path) -> Result<Option<Platform>, MetadataError> {
    let mut platform = None;
    for file in [files::SUMMARY, files::ENVIRONMENT] {
        let text = match std::fs::read_to_string(run_dir.join(file)) {
            Ok(text) => text,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(_) => return Err(MetadataError::Unreadable),
        };
        let metadata: Metadata = serde_json::from_str(&text).map_err(|_| MetadataError::Invalid)?;
        if let Some(observed) = metadata.platform {
            if platform.is_some_and(|prior| prior != observed) {
                return Err(MetadataError::Conflicting);
            }
            platform = Some(observed);
        }
    }
    Ok(platform)
}
