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
    /// The mostro-cli command-line client (profile `cli`). Selected by the
    /// profile, never by `--platform`: it is recorded so the journal, the
    /// actor storage and the provenance name the runtime they belong to.
    Cli,
}

impl std::fmt::Display for Platform {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Linux => "linux",
            Self::Web => "web",
            Self::Cli => "cli",
        })
    }
}

impl std::str::FromStr for Platform {
    type Err = String;
    /// Parses a `--platform` value: `cli` is a profile, not a platform an
    /// operator selects, so it is refused here.
    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "linux" => Ok(Self::Linux),
            "web" => Ok(Self::Web),
            _ => Err("platform must be linux or web".into()),
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    #[test]
    fn defaults_to_linux_and_refuses_unrecognized_platforms() {
        assert_eq!(Platform::default(), Platform::Linux);
        assert_eq!("web".parse::<Platform>(), Ok(Platform::Web));
        assert!("android".parse::<Platform>().is_err());
        assert!("desktop".parse::<Platform>().is_err());
    }

    #[test]
    fn the_cli_runtime_is_recorded_but_never_selected_as_a_platform() {
        assert!("cli".parse::<Platform>().is_err());
        assert_eq!(Platform::Cli.to_string(), "cli");
        let json = serde_json::to_string(&Platform::Cli).unwrap();
        assert_eq!(json, "\"cli\"");
        assert_eq!(
            serde_json::from_str::<Platform>(&json).unwrap(),
            Platform::Cli
        );
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

mod web_read;

pub mod storage;

mod native_retry;
