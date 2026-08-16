//! Run identifiers: sortable, filesystem-safe and unique across concurrent
//! developer runs on the same machine.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

/// Identifier of one Mortsom run: `<UTC timestamp>-<12 hex chars>`.
///
/// The identifier names a directory, so it is validated wherever it enters
/// the process — including deserialization, which would otherwise accept a
/// path traversal straight out of a summary file.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
#[serde(transparent)]
pub struct RunId(String);

impl<'de> Deserialize<'de> for RunId {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        text.parse().map_err(serde::de::Error::custom)
    }
}

impl RunId {
    /// Creates a new identifier for now.
    pub fn new() -> Self {
        Self::at(chrono::Utc::now(), uuid::Uuid::new_v4())
    }

    /// Deterministic constructor used by tests.
    pub fn at(now: chrono::DateTime<chrono::Utc>, nonce: uuid::Uuid) -> Self {
        // Twelve hex characters: enough that two runs started in the same
        // second do not silently share an evidence directory.
        let suffix = &nonce.simple().to_string()[..12];
        Self(format!("{}-{suffix}", now.format("%Y%m%dT%H%M%SZ")))
    }

    /// The identifier as a string slice.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Default for RunId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for RunId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl FromStr for RunId {
    type Err = String;

    /// Accepts only identifiers that are safe as a single path segment.
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let valid = !s.is_empty()
            && s.len() <= 64
            && s.chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_'));
        if valid {
            Ok(Self(s.to_owned()))
        } else {
            Err(format!("invalid run id `{s}`"))
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn deterministic_constructor_formats_timestamp_and_nonce() {
        let now = chrono::DateTime::parse_from_rfc3339("2026-08-15T13:45:00Z")
            .unwrap()
            .with_timezone(&chrono::Utc);
        let nonce = uuid::Uuid::parse_str("0123456789abcdef0123456789abcdef").unwrap();

        let id = RunId::at(now, nonce);

        assert_eq!(id.as_str(), "20260815T134500Z-0123456789ab");
        assert_eq!(id.to_string(), id.as_str());
    }

    #[test]
    fn new_ids_are_unique_and_parse_back() {
        let a = RunId::new();
        let b = RunId::new();

        assert_ne!(a, b);
        assert_eq!(a.as_str().parse::<RunId>().unwrap(), a);
    }

    #[test]
    fn unsafe_path_segments_are_rejected() {
        for bad in ["", "../x", "a/b", "a b", &"x".repeat(65)] {
            assert!(bad.parse::<RunId>().is_err(), "{bad:?}");
        }
    }

    #[test]
    fn serde_is_transparent() {
        let id: RunId = serde_json::from_str("\"20260815T134500Z-abcdef\"").unwrap();
        assert_eq!(
            serde_json::to_string(&id).unwrap(),
            "\"20260815T134500Z-abcdef\""
        );
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod validation_tests {
    use super::*;

    /// A run id becomes a path segment, so one that arrives inside a summary
    /// must pass the same validation as one parsed from the command line.
    #[test]
    fn deserializing_refuses_what_from_str_refuses() {
        assert!(serde_json::from_str::<RunId>("\"../run\"").is_err());
        assert!(serde_json::from_str::<RunId>("\"a/b\"").is_err());
        assert!(serde_json::from_str::<RunId>("\"20260816T000000Z-aaaaaa\"").is_ok());
    }

    /// Run ids name directories that sit next to each other; six hex chars
    /// is 24 bits, and a collision silently merges two runs' evidence.
    #[test]
    fn the_nonce_carries_more_than_twenty_four_bits() {
        let id = RunId::new();
        let nonce = id.as_str().rsplit_once('-').unwrap().1;
        assert!(nonce.len() >= 12, "{nonce} is too short to stay unique");
    }
}
