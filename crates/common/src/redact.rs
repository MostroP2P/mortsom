//! Text redaction for logs, artifacts and reports.
//!
//! Two layers protect secrets: typed [`crate::Secret`] values that cannot be
//! printed, and this scrubber that removes known secret values and
//! secret-shaped patterns from free text produced by external tools (adb
//! logcat, relay logs, WebDriver responses) before it is persisted.

use std::sync::LazyLock;

use regex::Regex;

use crate::secret::{REDACTED, Secret};

/// Patterns that are always redacted regardless of configuration.
static PATTERNS: LazyLock<Vec<Regex>> = LazyLock::new(|| {
    [
        // Nostr Wallet Connect URIs carry the wallet secret in the query.
        r#"nostr\+walletconnect://[^\s"'<>]+"#,
        // Bech32 private keys.
        r"\bnsec1[02-9ac-hj-np-z]{20,}\b",
        // Lightning macaroons and similar long hex tokens (>= 64 hex chars).
        r"\b[0-9a-fA-F]{64,}\b",
        // Base64-encoded macaroons and tokens (>= 80 base64 characters).
        r"[A-Za-z0-9+/_-]{80,}={0,2}",
        // BIP-39 mnemonics: 12 or more lowercase words in a row.
        r"\b(?:[a-z]{3,8}\s+){11,23}[a-z]{3,8}\b",
        // Explicit key=value style secrets.
        r"(?i)\b(secret|token|macaroon|password|mnemonic|seed)\s*[=:]\s*\S+",
    ]
    .into_iter()
    // A pattern that fails to compile is a programming error caught by
    // `every_builtin_pattern_compiles`; skipping it here avoids a panic in
    // production code paths.
    .filter_map(|pattern| Regex::new(pattern).ok())
    .collect()
});

/// Number of built-in patterns; used to prove they all compile.
#[cfg(test)]
const BUILTIN_PATTERN_COUNT: usize = 6;

/// Removes secret values and secret-shaped patterns from text.
///
/// The registered values are held as [`Secret`], so the scrubber itself
/// cannot print or leak what it exists to hide, and they are kept longest
/// first: a secret that contains another must be replaced before it, or the
/// tail of the longer one survives in the text.
#[derive(Debug, Default, Clone)]
pub struct Redactor {
    known: Vec<Secret>,
}

impl Redactor {
    /// Creates a redactor that only applies the built-in patterns.
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers a concrete secret value to scrub wherever it appears.
    /// Empty secrets are ignored so a placeholder cannot blank all text.
    pub fn with_secret(mut self, secret: &Secret) -> Self {
        self.add_secret(secret);
        self
    }

    /// Registers several secrets at once.
    ///
    /// A wallet backend carries more than one credential — a node has a
    /// macaroon and a certificate where NWC has a URI — and registering
    /// them one by one is how one gets forgotten.
    pub fn with_secrets<'a>(mut self, secrets: impl IntoIterator<Item = &'a Secret>) -> Self {
        for secret in secrets {
            self.add_secret(secret);
        }
        self
    }

    /// Registers a concrete secret value in place.
    pub fn add_secret(&mut self, secret: &Secret) {
        let value = secret.expose();
        if value.is_empty() || self.known.iter().any(|known| known.expose() == value) {
            return;
        }
        self.known.push(Secret::new(value.to_owned()));
        // Longest first, so an overlapping secret is never half-replaced.
        self.known
            .sort_by_key(|known| std::cmp::Reverse(known.expose().len()));
    }

    /// Redacts registered secrets in an explicitly public, typed metadata field.
    /// Free text must use `redact` so secret-shaped patterns are also removed.
    pub fn redact_known(&self, text: &str) -> String {
        let mut out = text.to_owned();
        for value in &self.known {
            out = out.replace(value.expose(), REDACTED);
        }
        out
    }

    /// Returns `text` with every known secret and secret-shaped pattern
    /// replaced by [`REDACTED`].
    pub fn redact(&self, text: &str) -> String {
        let mut out = self.redact_known(text);
        for pattern in PATTERNS.iter() {
            out = pattern.replace_all(&out, REDACTED).into_owned();
        }
        out
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn every_builtin_pattern_compiles() {
        assert_eq!(PATTERNS.len(), BUILTIN_PATTERN_COUNT);
    }

    #[test]
    fn known_secret_values_are_replaced() {
        let secret = Secret::new("hunter2-token");
        let redactor = Redactor::new().with_secret(&secret);

        let out = redactor.redact("value is hunter2-token, again hunter2-token");

        assert_eq!(out, format!("value is {REDACTED}, again {REDACTED}"));
    }

    #[test]
    fn empty_secret_does_not_blank_text() {
        let redactor = Redactor::new().with_secret(&Secret::new(""));

        assert_eq!(redactor.redact("untouched"), "untouched");
    }

    #[test]
    fn nwc_uri_is_redacted_by_pattern() {
        let redactor = Redactor::new();

        let out = redactor.redact(
            "connecting nostr+walletconnect://b889ff5b?relay=wss%3A%2F%2Fr&secret=71a8 done",
        );

        assert_eq!(out, format!("connecting {REDACTED} done"));
    }

    #[test]
    fn nsec_and_long_hex_are_redacted() {
        let redactor = Redactor::new();
        let hex = "a".repeat(64);

        let out = redactor.redact(&format!(
            "key nsec1qyq8wumn8ghj7un9d3shjtnwdaehgu3wvfskuep0qq macaroon {hex} end"
        ));

        assert!(!out.contains("nsec1"));
        assert!(!out.contains(&hex));
        assert!(out.ends_with(" end"));
    }

    #[test]
    fn base64_macaroons_are_redacted() {
        let redactor = Redactor::new();
        let macaroon = "AgEDbG5kAvgBAwoQ".repeat(6);

        let out = redactor.redact(&format!("macaroon {macaroon} end"));

        assert!(!out.contains(&macaroon));
        assert!(out.ends_with(" end"));
    }

    #[test]
    fn mnemonic_phrases_are_redacted() {
        let redactor = Redactor::new();
        let phrase =
            "abandon ability able about above absent absorb abstract absurd abuse access accident";

        let out = redactor.redact(&format!("seed words: {phrase}."));

        assert!(!out.contains("abandon ability"));
    }

    #[test]
    fn short_hex_such_as_order_ids_survives() {
        let redactor = Redactor::new();
        let order_id = "3f2a9c1e-5b7d-4e8a-9c0f-1a2b3c4d5e6f";

        assert_eq!(redactor.redact(order_id), order_id);
    }

    #[test]
    fn labelled_secrets_are_redacted() {
        let redactor = Redactor::new();

        let out = redactor.redact("password=abc token: xyz other=fine");

        assert_eq!(out, format!("{REDACTED} {REDACTED} other=fine"));
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod ordering_tests {
    use super::*;

    /// A secret that contains another must be redacted whole. Replacing the
    /// shorter one first would leave the rest of the longer one in the text.
    #[test]
    fn an_overlapping_longer_secret_is_redacted_whole() {
        let redactor = Redactor::new()
            .with_secret(&Secret::new("abc123".to_owned()))
            .with_secret(&Secret::new("abc123def456".to_owned()));

        let clean = redactor.redact("token abc123def456 end");

        assert!(!clean.contains("def456"), "{clean}");
        assert_eq!(clean, format!("token {REDACTED} end"));
    }

    /// The redactor holds the very values it exists to hide, so its own
    /// debug output must not print them.
    #[test]
    fn the_redactor_never_prints_the_secrets_it_holds() {
        let redactor = Redactor::new().with_secret(&Secret::new("hunter2xyz".to_owned()));

        assert!(!format!("{redactor:?}").contains("hunter2xyz"));
    }
}
