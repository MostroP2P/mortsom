//! Capabilities an application generation supports (spec §12.2). A scenario
//! declares what it requires; a profile lacking any of it is reported as
//! skipped with a reason, never passed and never silently omitted.

use std::collections::BTreeSet;
use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

/// A business capability of the app under test.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Capability {
    /// First-launch onboarding with identity import or generation.
    Onboarding,
    /// Selecting a custom Mostro node by pubkey.
    CustomNodeSelection,
    /// Configuring the local relay.
    LocalRelayConfiguration,
    /// Nostr Wallet Connect setup through settings.
    Nwc,
    /// Creating fixed-price orders.
    CreateOrder,
    /// Creating orders priced at the market rate.
    MarketPriceOrder,
    /// Creating orders with a fiat range and taking them at an amount.
    RangeOrder,
    /// Discovering and taking an order.
    TakeOrder,
    /// Paying the hold invoice through the connected wallet.
    HoldInvoicePayment,
    /// Handing the hold invoice to an outside payer and accepting a buyer
    /// invoice typed in by hand — the controls a run uses when the wallet
    /// is a node Mortsom holds rather than one the app is connected to.
    ExternalWallet,
    /// Submitting a buyer invoice.
    BuyerInvoice,
    /// Reporting fiat sent.
    FiatSent,
    /// Releasing sats.
    Release,
    /// Observing the completed state.
    CompletedObservation,
    /// Cancelling an order.
    Cancel,
    /// Opening a dispute.
    Dispute,
    /// Rating a counterpart.
    Rating,
    /// System notifications that route to an order.
    Notifications,
    /// Deep links into orders.
    DeepLinks,
    /// A visible test-environment marker.
    TestEnvironmentMarker,
}

impl Capability {
    /// Every capability, in declaration order.
    pub const ALL: [Capability; 20] = [
        Capability::Onboarding,
        Capability::CustomNodeSelection,
        Capability::LocalRelayConfiguration,
        Capability::Nwc,
        Capability::CreateOrder,
        Capability::MarketPriceOrder,
        Capability::RangeOrder,
        Capability::TakeOrder,
        Capability::HoldInvoicePayment,
        Capability::ExternalWallet,
        Capability::BuyerInvoice,
        Capability::FiatSent,
        Capability::Release,
        Capability::CompletedObservation,
        Capability::Cancel,
        Capability::Dispute,
        Capability::Rating,
        Capability::Notifications,
        Capability::DeepLinks,
        Capability::TestEnvironmentMarker,
    ];

    /// Stable kebab-case name.
    pub fn as_str(self) -> &'static str {
        match self {
            Capability::Onboarding => "onboarding",
            Capability::CustomNodeSelection => "custom-node-selection",
            Capability::LocalRelayConfiguration => "local-relay-configuration",
            Capability::Nwc => "nwc",
            Capability::CreateOrder => "create-order",
            Capability::MarketPriceOrder => "market-price-order",
            Capability::RangeOrder => "range-order",
            Capability::TakeOrder => "take-order",
            Capability::HoldInvoicePayment => "hold-invoice-payment",
            Capability::ExternalWallet => "external-wallet",
            Capability::BuyerInvoice => "buyer-invoice",
            Capability::FiatSent => "fiat-sent",
            Capability::Release => "release",
            Capability::CompletedObservation => "completed-observation",
            Capability::Cancel => "cancel",
            Capability::Dispute => "dispute",
            Capability::Rating => "rating",
            Capability::Notifications => "notifications",
            Capability::DeepLinks => "deep-links",
            Capability::TestEnvironmentMarker => "test-environment-marker",
        }
    }
}

impl fmt::Display for Capability {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for Capability {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Capability::ALL
            .into_iter()
            .find(|c| c.as_str() == s)
            .ok_or_else(|| format!("unknown capability `{s}`"))
    }
}

/// A set of capabilities.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CapabilitySet(BTreeSet<Capability>);

impl CapabilitySet {
    /// Builds a set.
    pub fn new(capabilities: impl IntoIterator<Item = Capability>) -> Self {
        Self(capabilities.into_iter().collect())
    }

    /// True when `capability` is supported.
    pub fn supports(&self, capability: Capability) -> bool {
        self.0.contains(&capability)
    }

    /// Capabilities in `required` that this set lacks, in order.
    pub fn missing(&self, required: &[Capability]) -> Vec<Capability> {
        required
            .iter()
            .copied()
            .filter(|c| !self.supports(*c))
            .collect()
    }

    /// Iterates the capabilities.
    pub fn iter(&self) -> impl Iterator<Item = Capability> + '_ {
        self.0.iter().copied()
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn names_round_trip() {
        for c in Capability::ALL {
            assert_eq!(c.as_str().parse::<Capability>().unwrap(), c);
        }
        assert!("teleport".parse::<Capability>().is_err());
    }

    #[test]
    fn missing_lists_gaps_in_required_order() {
        let set = CapabilitySet::new([Capability::Onboarding, Capability::Nwc]);
        assert!(set.supports(Capability::Nwc));
        assert_eq!(
            set.missing(&[Capability::Release, Capability::Nwc, Capability::Dispute]),
            vec![Capability::Release, Capability::Dispute]
        );
        assert_eq!(set.iter().count(), 2);
    }
}
