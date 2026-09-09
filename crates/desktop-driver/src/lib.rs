//! Linux AT-SPI and WebDriver clients. All business actions use public UI.

use serde::{Deserialize, Serialize};

/// Runtime selected for both actors.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Platform {
    /// Native Flutter desktop.
    #[default]
    Linux,
    /// Flutter Web in isolated browser profiles.
    Web,
    /// Existing Android implementation (frozen).
    Android,
}

impl std::fmt::Display for Platform {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Linux => "linux",
            Self::Web => "web",
            Self::Android => "android",
        })
    }
}

impl std::str::FromStr for Platform {
    type Err = String;
    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "linux" => Ok(Self::Linux),
            "web" => Ok(Self::Web),
            "android" => Ok(Self::Android),
            _ => Err("platform must be linux, web or android".into()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn defaults_to_linux_and_refuses_unrecognized_platforms() {
        assert_eq!(Platform::default(), Platform::Linux);
        assert_eq!("web".parse::<Platform>(), Ok(Platform::Web));
        assert_eq!("android".parse::<Platform>(), Ok(Platform::Android));
        assert!("desktop".parse::<Platform>().is_err());
    }
}
pub mod linux;
pub mod native;

/// Escapes public tree values for normalized XML evidence.
pub fn xml_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}
pub mod server;
pub mod web;

/// Focus and read-back verified text entry.
pub mod text_entry;

mod web_input;

pub mod storage;

mod native_retry;
