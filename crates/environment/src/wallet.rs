//! Read-only wallet probing: reachability and capability summary, over NWC
//! or a local LND node. Never creates an invoice or pays anything, and never
//! returns a URI, a macaroon or a certificate.

use std::str::FromStr;
use std::time::Duration;

use mortsom_common::{Redactor, Secret};

use crate::lnd::LndWallet;
use nostr_sdk::prelude::NostrWalletConnectUri;
use nwc::NostrWalletConnect;
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Redacted description of a wallet, safe for reports and artifacts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WalletProbe {
    /// Opaque label chosen by Mortsom (`alice`, `bob`).
    pub label: String,
    /// Wallet alias reported by the service, if any.
    pub alias: Option<String>,
    /// Network reported by the service, if any.
    pub network: Option<String>,
    /// NWC methods the wallet supports.
    pub methods: Vec<String>,
    /// Spendable balance in sats. On a node this is outbound liquidity.
    pub balance_sats: u64,
    /// Inbound liquidity in sats, when the backend can report it.
    ///
    /// A node with no inbound liquidity cannot be paid, which on a freshly
    /// created Polar network is the usual reason a trade dies halfway
    /// through. NWC exposes nothing equivalent, so this stays `None` there.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inbound_sats: Option<u64>,
}

/// Failures while probing; messages never contain the URI.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum WalletProbeError {
    /// The URI is empty (unresolved secret) or malformed.
    #[error("{label}: NWC URI is invalid or unresolved")]
    InvalidUri {
        /// Wallet label.
        label: String,
    },
    /// The wallet service did not answer in time.
    #[error("{label}: NWC {operation} timed out after {timeout:?}")]
    Timeout {
        /// Wallet label.
        label: String,
        /// Operation that timed out.
        operation: &'static str,
        /// Deadline used.
        timeout: Duration,
    },
    /// The backend reported a failure. The message is already labelled and
    /// scrubbed by the backend that produced it.
    #[error("{0}")]
    Backend(String),
    /// The wallet service returned an error.
    #[error("{label}: NWC {operation} failed: {message}")]
    Failed {
        /// Wallet label.
        label: String,
        /// Operation that failed.
        operation: &'static str,
        /// Error text from the wallet, sanitized of the URI.
        message: String,
    },
}

/// Probes an NWC wallet with `get_info` and `get_balance` only.
pub async fn probe_nwc(
    label: &str,
    uri: &Secret,
    timeout: Duration,
) -> Result<WalletProbe, WalletProbeError> {
    let parsed = NostrWalletConnectUri::from_str(uri.expose()).map_err(|_| {
        WalletProbeError::InvalidUri {
            label: label.to_owned(),
        }
    })?;
    let redactor = redactor_for(uri, &parsed);
    let wallet = NostrWalletConnect::builder(parsed).timeout(timeout).build();
    let info = tokio::time::timeout(timeout, wallet.get_info())
        .await
        .map_err(|_| timeout_error(label, "get_info", timeout))?
        .map_err(|e| failed(label, "get_info", e.to_string(), &redactor))?;
    let balance_msats = tokio::time::timeout(timeout, wallet.get_balance())
        .await
        .map_err(|_| timeout_error(label, "get_balance", timeout))?
        .map_err(|e| failed(label, "get_balance", e.to_string(), &redactor))?;
    Ok(WalletProbe {
        label: label.to_owned(),
        alias: info.alias,
        network: info.network,
        methods: info.methods.iter().map(|m| m.to_string()).collect(),
        balance_sats: balance_msats.balance / 1000,
        inbound_sats: None,
    })
}

/// What Mortsom asks of an LND node. Unlike NWC, where the wallet service
/// advertises its own method list, these are fixed: they are the calls this
/// backend makes.
pub const LND_METHODS: [&str; 3] = ["make_invoice", "pay_invoice", "lookup_invoice"];

/// Probes an LND node with `get_info` and `channel_balance` only.
pub async fn probe_lnd(wallet: &LndWallet) -> Result<WalletProbe, WalletProbeError> {
    let label = wallet.label().to_owned();
    let (alias, network) = wallet
        .info()
        .await
        .map_err(|e| WalletProbeError::Backend(e.to_string()))?;
    let (outbound, inbound) = wallet
        .channel_balance()
        .await
        .map_err(|e| WalletProbeError::Backend(e.to_string()))?;
    Ok(WalletProbe {
        label,
        alias: (!alias.is_empty()).then_some(alias),
        network: (!network.is_empty()).then_some(network),
        methods: LND_METHODS.iter().map(|m| (*m).to_owned()).collect(),
        balance_sats: outbound,
        inbound_sats: Some(inbound),
    })
}

fn timeout_error(label: &str, operation: &'static str, timeout: Duration) -> WalletProbeError {
    WalletProbeError::Timeout {
        label: label.to_owned(),
        operation,
        timeout,
    }
}

/// A redactor that knows the whole URI and each sensitive component, so an
/// error text that embeds only the bare secret key is still scrubbed.
fn redactor_for(uri: &Secret, parsed: &NostrWalletConnectUri) -> Redactor {
    Redactor::new()
        .with_secret(uri)
        .with_secret(&Secret::new(parsed.secret.to_secret_hex()))
        .with_secret(&Secret::new(parsed.public_key.to_hex()))
}

fn failed(
    label: &str,
    operation: &'static str,
    message: String,
    redactor: &Redactor,
) -> WalletProbeError {
    WalletProbeError::Failed {
        label: label.to_owned(),
        operation,
        message: redactor.redact(&message),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn empty_or_malformed_uri_is_rejected_without_network() {
        for value in ["", "https://example.com", "nostr+walletconnect://nope"] {
            let err = probe_nwc("alice", &Secret::new(value), Duration::from_secs(1))
                .await
                .unwrap_err();
            assert_eq!(
                err,
                WalletProbeError::InvalidUri {
                    label: "alice".into()
                }
            );
        }
    }

    const URI: &str = "nostr+walletconnect://b889ff5b1513b641e2a139f661a661364979c5beee91842f8f0ef42ab558e9d4?relay=wss%3A%2F%2Frelay.damus.io&secret=71a8c14c1407c113601079c4302dab36460f0ccd0ad506f1f2dc73b5100e4f3c";

    #[test]
    fn failure_messages_never_include_the_uri_or_its_components() {
        let uri = Secret::new(URI);
        let parsed = NostrWalletConnectUri::from_str(URI).unwrap();
        let redactor = redactor_for(&uri, &parsed);

        let whole = failed("bob", "get_info", format!("cannot reach {URI}"), &redactor);
        let bare_secret = failed(
            "bob",
            "get_info",
            "bad key 71a8c14c1407c113601079c4302dab36460f0ccd0ad506f1f2dc73b5100e4f3c".into(),
            &redactor,
        );
        let bare_pubkey = failed(
            "bob",
            "get_info",
            "service b889ff5b1513b641e2a139f661a661364979c5beee91842f8f0ef42ab558e9d4 offline"
                .into(),
            &redactor,
        );

        for err in [whole, bare_secret, bare_pubkey] {
            let text = err.to_string();
            assert!(!text.contains("71a8c14c"), "{text}");
            assert!(!text.contains("b889ff5b"), "{text}");
            assert!(text.contains("<redacted>"), "{text}");
        }
    }

    #[test]
    fn timeout_message_names_operation() {
        let err = timeout_error("alice", "get_balance", Duration::from_secs(3));

        assert_eq!(err.to_string(), "alice: NWC get_balance timed out after 3s");
    }
}
