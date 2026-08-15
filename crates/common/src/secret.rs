//! A string that must never leak into logs, artifacts or reports.

use std::fmt;

use serde::{Deserialize, Serialize, Serializer};
use zeroize::{Zeroize, ZeroizeOnDrop};

/// Placeholder written wherever a secret would otherwise appear.
pub const REDACTED: &str = "<redacted>";

/// A secret string (NWC URI, mnemonic, private key, token).
///
/// Deserializes transparently from a plain string, but `Debug`, `Display`
/// and `Serialize` always emit [`REDACTED`]. The only way to read the value
/// is [`Secret::expose`], which keeps every intentional exposure greppable.
/// The buffer is zeroized on drop.
#[derive(Clone, PartialEq, Eq, Deserialize, Zeroize, ZeroizeOnDrop)]
#[serde(transparent)]
pub struct Secret(String);

impl Secret {
    /// Wraps a value that must be protected from now on.
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// Returns the protected value. Every call site is an intentional exposure.
    pub fn expose(&self) -> &str {
        &self.0
    }

    /// True when the wrapped value is empty (an unresolved placeholder).
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(REDACTED)
    }
}

impl fmt::Display for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(REDACTED)
    }
}

impl Serialize for Secret {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(REDACTED)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn debug_and_display_never_show_the_value() {
        let secret = Secret::new("nostr+walletconnect://abc?relay=x&secret=y");

        assert_eq!(format!("{secret:?}"), REDACTED);
        assert_eq!(secret.to_string(), REDACTED);
    }

    #[test]
    fn serialize_writes_the_placeholder() {
        let secret = Secret::new("nsec1qqq");

        let json = serde_json::to_string(&secret).unwrap();

        assert_eq!(json, format!("\"{REDACTED}\""));
    }

    #[test]
    fn deserialize_is_transparent_and_expose_returns_the_value() {
        let secret: Secret = serde_json::from_str("\"top secret\"").unwrap();

        assert_eq!(secret.expose(), "top secret");
        assert!(!secret.is_empty());
    }

    #[test]
    fn empty_secret_reports_empty() {
        assert!(Secret::new("").is_empty());
    }

    #[test]
    fn debug_of_a_struct_containing_a_secret_is_redacted() {
        #[derive(Debug)]
        #[allow(dead_code)]
        struct Wallet {
            uri: Secret,
        }
        let wallet = Wallet {
            uri: Secret::new("nostr+walletconnect://leak"),
        };

        let rendered = format!("{wallet:?}");

        assert!(!rendered.contains("leak"));
        assert!(rendered.contains(REDACTED));
    }
}
