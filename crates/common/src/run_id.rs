//! Run identifiers: sortable, filesystem-safe and unique across concurrent
//! developer runs on the same machine.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

/// Identifier of one Mortsom run: `<UTC timestamp>-<6 hex chars>`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct RunId(String);

impl RunId {
    /// Creates a new identifier for now.
    pub fn new() -> Self {
        Self::at(chrono::Utc::now(), uuid::Uuid::new_v4())
    }

    /// Deterministic constructor used by tests.
    pub fn at(now: chrono::DateTime<chrono::Utc>, nonce: uuid::Uuid) -> Self {
        let suffix = &nonce.simple().to_string()[..6];
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

        assert_eq!(id.as_str(), "20260815T134500Z-012345");
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
