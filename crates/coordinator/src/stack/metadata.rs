//! Public environment facts and explicitly declared daemon expectations.
use serde::{Deserialize, Serialize};

use super::StackError;

/// Public stack facts captured before client preparation; credential paths are excluded.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct StackSnapshot {
    /// Daemon public identity obtained from the public descriptor.
    pub daemon_pubkey: String,
    /// Host-reachable run relay.
    pub relay_url: String,
    /// Actor roles mapped to local wallet service endpoints.
    pub wallet_endpoints: std::collections::BTreeMap<String, String>,
    /// Actual container image identity.
    pub daemon_image: DaemonImage,
    /// Expectations which still require scenario verification.
    pub declared_limits: DeclaredLimits,
}

impl StackSnapshot {
    /// Rejects active services which differ from the inspected stack facts.
    pub fn validate_config(&self, config: &crate::Config) -> Result<(), StackError> {
        let key = nostr_sdk::prelude::PublicKey::parse(&self.daemon_pubkey)
            .map_err(|_| invalid("stack identity binding"))?;
        if config.mobile.mostro_pubkey() != Some(key)
            || config.mostro.relays != [self.relay_url.clone()]
            || self.relay_url != format!("ws://127.0.0.1:{}", config.relay.host_port)
        {
            return Err(invalid("daemon or relay binding"));
        }
        for (actor, wallet) in [
            ("alice", &config.wallets.alice),
            ("bob", &config.wallets.bob),
        ] {
            let mortsom_environment::WalletConfig::Lnd { host, .. } = wallet else {
                return Err(invalid("persistent stack wallet backend"));
            };
            if self.wallet_endpoints.get(actor) != Some(host) {
                return Err(invalid("wallet role binding"));
            }
        }
        Ok(())
    }
}

/// Image identity observed on the running daemon container.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct DaemonImage {
    /// Docker platform image ID, distinct from a registry manifest digest.
    pub image_id: String,
    /// Container creation reference, which may be a mutable tag.
    pub reference: String,
    /// Manifest digest only when the creation reference contains it.
    pub repository_digest: Option<String>,
}

impl DaemonImage {
    pub(crate) fn validate(&self) -> Result<(), StackError> {
        let declared_digest = self.reference.rsplit_once('@').map(|(_, hash)| hash);
        if !digest(&self.image_id)
            || self.reference.is_empty()
            || self.reference.chars().any(char::is_whitespace)
            || self.reference.starts_with('@')
            || declared_digest.is_some_and(|value| !digest(value))
            || declared_digest != self.repository_digest.as_deref()
        {
            return Err(invalid("daemon image identity"));
        }
        Ok(())
    }
}

/// Limits the stack declares in `stack/limits.toml`, never inferred from the daemon version.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct DeclaredLimits {
    /// Time before an untaken order expires.
    pub order_expiration_secs: u64,
    /// Lifetime of an unpaid seller hold invoice.
    pub hold_invoice_expiry_secs: u64,
    /// Time allowed to supply the buyer payout invoice.
    pub buyer_invoice_deadline_secs: u64,
    /// Optional declared fiat-payment deadline.
    pub fiat_window_secs: Option<u64>,
    /// Required remaining buyer invoice lifetime.
    pub payout_invoice_min_expiry_secs: u64,
    /// Optional ceiling on unresolved payouts to one destination.
    pub max_inflight_payouts_per_destination: Option<u32>,
    /// Optional declared concurrent payout dispatch capacity.
    pub payout_dispatch_permits: Option<u32>,
}

impl DeclaredLimits {
    /// The limits scenarios about payouts drive, in the kit's terms.
    pub fn payout_limits(&self) -> mortsom_scenario_kit::PayoutLimits {
        mortsom_scenario_kit::PayoutLimits {
            payout_invoice_min_expiry_secs: Some(self.payout_invoice_min_expiry_secs),
            max_inflight_payouts_per_destination: self.max_inflight_payouts_per_destination,
        }
    }
}

impl DeclaredLimits {
    pub(crate) fn validate(&self) -> Result<(), StackError> {
        let durations = [
            self.order_expiration_secs,
            self.hold_invoice_expiry_secs,
            self.buyer_invoice_deadline_secs,
            self.payout_invoice_min_expiry_secs,
        ];
        if durations.contains(&0)
            || self.fiat_window_secs == Some(0)
            || self.max_inflight_payouts_per_destination == Some(0)
            || self.payout_dispatch_permits == Some(0)
        {
            return Err(invalid("declared daemon limits"));
        }
        Ok(())
    }
}

fn digest(value: &str) -> bool {
    value
        .strip_prefix("sha256:")
        .is_some_and(|hash| hash.len() == 64 && hash.bytes().all(|byte| byte.is_ascii_hexdigit()))
}

fn invalid(field: &str) -> StackError {
    StackError::Configuration(format!("invalid public {field}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_tag_cannot_claim_a_manifest_digest_and_an_immutable_reference_must_match() {
        let mut image = DaemonImage {
            image_id: format!("sha256:{}", "a".repeat(64)),
            reference: "mostrop2p/mostro:latest".into(),
            repository_digest: None,
        };
        assert!(image.validate().is_ok());
        let digest = format!("sha256:{}", "b".repeat(64));
        image.repository_digest = Some(digest.clone());
        assert!(image.validate().is_err());
        image.reference = format!("mostrop2p/mostro@{digest}");
        assert!(image.validate().is_ok());
        image.repository_digest = Some(image.image_id.clone());
        assert!(image.validate().is_err());
        image.reference = format!("@{digest}");
        assert!(image.validate().is_err());
    }
}
