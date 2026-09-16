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
    /// How that image was selected, and the exact commit when built here.
    pub daemon_source: DaemonProvenance,
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

/// How the daemon image was selected. Together with [`DaemonImage`] this
/// names exactly which daemon a run tested.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum DaemonProvenance {
    /// Mortsom's pinned release, pulled by tag and pinned by digest.
    PinnedImage,
    /// A published image the operator named with `--mostro-image`.
    Image {
        /// The reference as the operator gave it.
        requested: String,
    },
    /// A Mostro branch or pull request built here with `--mostro-ref`.
    GitRef(crate::git_source::RemoteSelection),
}

impl DaemonProvenance {
    /// Rejects a record whose source and image identity disagree: a pulled
    /// image carries an official digest, a built one its commit tag.
    pub(crate) fn validate(&self, image: &DaemonImage) -> Result<(), StackError> {
        let consistent = match self {
            Self::PinnedImage => image.repository_digest.is_some(),
            Self::Image { requested } => {
                !requested.is_empty()
                    && !requested.chars().any(char::is_whitespace)
                    && image.repository_digest.is_some()
            }
            Self::GitRef(selection) => {
                valid_selection(selection)
                    && image.repository_digest.is_none()
                    && image.reference == super::image::commit_tag(&selection.revision)
            }
        };
        if !consistent {
            return Err(invalid("daemon source provenance"));
        }
        Ok(())
    }
}

impl std::fmt::Display for DaemonProvenance {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::PinnedImage => formatter.write_str("pinned release"),
            Self::Image { requested } => write!(formatter, "image {requested}"),
            Self::GitRef(selection) => write!(
                formatter,
                "{} ({} at {})",
                selection.requested, selection.git_ref, selection.revision
            ),
        }
    }
}

fn valid_selection(selection: &crate::git_source::RemoteSelection) -> bool {
    selection.repository == crate::mostro_source::REPOSITORY
        && selection.revision.len() == 40
        && selection
            .revision
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit())
        && crate::mostro_source::Selector::parse(&selection.requested)
            .is_ok_and(|selector| selector.git_ref() == selection.git_ref)
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
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::git_source::RemoteSelection;

    const REVISION: &str = "0123456789abcdef0123456789abcdef01234567";

    fn built() -> (DaemonImage, DaemonProvenance) {
        let image = DaemonImage {
            image_id: format!("sha256:{}", "a".repeat(64)),
            reference: super::super::image::commit_tag(REVISION),
            repository_digest: None,
        };
        let source = DaemonProvenance::GitRef(RemoteSelection {
            repository: crate::mostro_source::REPOSITORY.into(),
            requested: "https://github.com/MostroP2P/mostro/pull/963".into(),
            git_ref: "refs/pull/963/head".into(),
            revision: REVISION.into(),
        });
        (image, source)
    }

    #[test]
    fn a_built_daemon_records_its_commit_under_its_own_tag_and_no_registry_digest() {
        // Arrange
        let (image, source) = built();

        // Act / Assert
        assert!(source.validate(&image).is_ok());
        let encoded = serde_json::to_value(&source).unwrap();
        assert_eq!(encoded["kind"], "git-ref");
        assert_eq!(encoded["revision"], REVISION);
        assert_eq!(
            serde_json::from_value::<DaemonProvenance>(encoded).unwrap(),
            source
        );
    }

    #[test]
    fn a_built_daemon_record_that_disagrees_with_itself_is_refused() {
        let (image, source) = built();
        let DaemonProvenance::GitRef(selection) = &source else {
            unreachable!()
        };
        let mut invented_digest = image.clone();
        invented_digest.repository_digest = Some(format!("sha256:{}", "b".repeat(64)));
        invented_digest.reference = format!("mortsom/mostro@sha256:{}", "b".repeat(64));
        let mut other_tag = image.clone();
        other_tag.reference = "mortsom/mostro:latest".into();
        for (image, selection) in [
            (invented_digest, selection.clone()),
            (other_tag, selection.clone()),
            (
                image.clone(),
                RemoteSelection {
                    repository: "https://github.com/someone/mostro.git".into(),
                    ..selection.clone()
                },
            ),
            (
                image.clone(),
                RemoteSelection {
                    git_ref: "refs/heads/main".into(),
                    ..selection.clone()
                },
            ),
        ] {
            assert!(
                DaemonProvenance::GitRef(selection.clone())
                    .validate(&image)
                    .is_err(),
                "{image:?} {selection:?}"
            );
        }
    }

    #[test]
    fn a_pulled_daemon_must_carry_its_official_digest() {
        let digest = format!("sha256:{}", "b".repeat(64));
        let image = DaemonImage {
            image_id: format!("sha256:{}", "a".repeat(64)),
            reference: format!("mostrop2p/mostro@{digest}"),
            repository_digest: Some(digest),
        };
        let requested = DaemonProvenance::Image {
            requested: "mostrop2p/mostro:v0.18.8".into(),
        };
        assert!(DaemonProvenance::PinnedImage.validate(&image).is_ok());
        assert!(requested.validate(&image).is_ok());
        let (built, _) = built();
        assert!(DaemonProvenance::PinnedImage.validate(&built).is_err());
        assert!(requested.validate(&built).is_err());
    }

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
