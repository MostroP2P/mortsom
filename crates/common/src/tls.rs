//! Process-wide TLS provider selection.
//!
//! `rustls` refuses to guess a cryptographic provider when more than one is
//! compiled in, and both `ring` (through the nostr stack) and `aws-lc-rs`
//! (through `reqwest`) reach it here. Without an explicit choice the first
//! `wss://` connection panics inside a background task, which is how a
//! missing relay or wallet used to look: a stack trace and a timeout.

/// Installs the process-level provider once. Returns true when this call
/// installed it, false when one was already in place. Safe to call from
/// every entry point, including tests running in the same process.
pub fn install_crypto_provider() -> bool {
    rustls::crypto::aws_lc_rs::default_provider()
        .install_default()
        .is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn installing_twice_is_not_an_error() {
        // Whichever call ran first, a provider is available afterwards and
        // a second call reports that it did not install one.
        install_crypto_provider();
        assert!(!install_crypto_provider());
        assert!(rustls::crypto::CryptoProvider::get_default().is_some());
    }
}
