//! Scenario registry: scenarios self-register with `inventory`; the runner
//! never keeps a hand-maintained list.

use std::future::Future;
use std::pin::Pin;

use mortsom_app_contract::Capability;

use crate::context::{Context, ScenarioError};

/// A scenario body.
pub type ScenarioFuture<'a> = Pin<Box<dyn Future<Output = Result<(), ScenarioError>> + Send + 'a>>;
/// Function pointer that runs a scenario.
pub type ScenarioFn = for<'a> fn(&'a Context) -> ScenarioFuture<'a>;

/// A registered scenario.
pub struct ScenarioDef {
    /// Unique name (`happy_sell`).
    pub name: &'static str,
    /// Suite tags (`smoke`, `regression`, `recovery`, `adversarial`).
    pub tags: &'static [&'static str],
    /// Capabilities the profile must support.
    pub requires: &'static [Capability],
    /// Deadline for one attempt.
    pub timeout_secs: u64,
    /// True when the scenario moves sats; it then needs mainnet clearance.
    pub moves_value: bool,
    /// True when the scenario is *about* the app's own connected wallet --
    /// cancelling from the pay screen instead of paying through it -- so a
    /// run whose payer is a node Mortsom holds never takes that path and
    /// must skip the scenario rather than report a pass for a path it did
    /// not exercise.
    pub needs_app_wallet: bool,
    /// True when the scenario needs a dispute solver registered with the
    /// daemon: a run that cannot provide one skips it rather than fail it.
    pub needs_solver: bool,
    /// Interruptions the scenario must cause beyond what the app offers; a
    /// run whose clients or relay cannot cause one skips it rather than
    /// fail it.
    pub needs: &'static [Need],
    /// Body.
    pub run: ScenarioFn,
}

inventory::collect!(ScenarioDef);

/// An interruption a scenario causes that lies outside the app itself: in
/// the client platform or in the run's relay.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Need {
    /// Sending the app to the background and bringing it back.
    Background,
    /// Cutting the client's network and restoring it.
    Connectivity,
    /// Rotating the device.
    Rotation,
    /// Restarting the run relay, or replaying events into it.
    RelayFaults,
}

impl Need {
    /// Stable kebab-case name, as reports print it.
    pub fn as_str(self) -> &'static str {
        match self {
            Need::Background => "background",
            Need::Connectivity => "connectivity",
            Need::Rotation => "rotation",
            Need::RelayFaults => "relay-faults",
        }
    }
}

/// Every registered scenario, sorted by name.
pub fn all() -> Vec<&'static ScenarioDef> {
    let mut defs: Vec<&'static ScenarioDef> = inventory::iter::<ScenarioDef>.into_iter().collect();
    defs.sort_by_key(|d| d.name);
    defs
}

/// Finds a scenario by name.
pub fn find(name: &str) -> Option<&'static ScenarioDef> {
    all().into_iter().find(|d| d.name == name)
}

/// Which side of a trade this run drives by hand.
///
/// A run mixes backends per actor: `[wallets]` names Alice's and Bob's
/// wallets separately, and a trade with one of each is a configuration the
/// loader accepts. Collapsing that into a single run-wide flag loses the
/// half that still pays through the app, so what a scenario needs is
/// resolved against the actor that performs each step.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ManualPayments {
    /// Alice — the seller, who pays the hold invoice.
    pub alice: bool,
    /// Bob — the buyer, whose payout invoice the app must accept.
    pub bob: bool,
}

impl ManualPayments {
    /// Both actors pay through the app's own connected wallet.
    pub const NONE: Self = Self {
        alice: false,
        bob: false,
    };
    /// Both actors are driven from nodes Mortsom holds.
    pub const BOTH: Self = Self {
        alice: true,
        bob: true,
    };

    /// Whether either actor is driven through the app's manual controls.
    pub fn any(self) -> bool {
        self.alice || self.bob
    }
}

/// The capabilities a scenario needs from the app on this run's wallet
/// backends.
///
/// A scenario names the journey, not the plumbing: "fund the escrow" is
/// the same step whether the app pays it through its own connected wallet
/// or Mortsom pays it from a node it holds and types the buyer invoice
/// into the app by hand. Which controls that step actually drives is a
/// property of the run, not of the scenario, so scenarios declare the NWC
/// path and this resolves it -- rather than making every scenario carry
/// both and the gate check requirements the run will never exercise.
///
/// The resolution is per actor, because the run's wallets are: Alice pays
/// the hold invoice, so `hold-invoice-payment` becomes `external-wallet`
/// only when *her* wallet is a node Mortsom holds; Bob hands in the payout
/// invoice, so his node adds the manual invoice control to
/// `buyer-invoice`; and `nwc` -- the app's own wallet connection -- is
/// still reached as long as either actor pays through the app.
pub fn required_on(requires: &[Capability], manual: ManualPayments) -> Vec<Capability> {
    if !manual.any() {
        return requires.to_vec();
    }
    let mut resolved = Vec::with_capacity(requires.len() + 1);
    // Both wallet capabilities map onto the same manual controls; asking
    // for them twice would report one missing capability as two.
    let mut push = |capability: Capability| {
        if !resolved.contains(&capability) {
            resolved.push(capability);
        }
    };
    for capability in requires {
        match capability {
            // Nobody's app connects a wallet only when nobody's app pays.
            Capability::Nwc if manual.alice && manual.bob => push(Capability::ExternalWallet),
            Capability::HoldInvoicePayment if manual.alice => push(Capability::ExternalWallet),
            // The app still needs its invoice control, and it must accept
            // the invoice our node issued rather than making its own.
            Capability::BuyerInvoice if manual.bob => {
                push(Capability::BuyerInvoice);
                push(Capability::ExternalWallet);
            }
            other => push(*other),
        }
    }
    resolved
}

/// Selects scenarios by explicit names and/or a tag. Unknown names are an
/// error, never silently ignored.
pub fn select(names: &[String], tag: Option<&str>) -> Result<Vec<&'static ScenarioDef>, String> {
    let mut selected = Vec::new();
    for name in names {
        let def = find(name).ok_or_else(|| format!("unknown scenario `{name}`"))?;
        // A name repeated on the command line runs the scenario once.
        if !selected.iter().any(|d: &&ScenarioDef| d.name == def.name) {
            selected.push(def);
        }
    }
    if let Some(tag) = tag {
        let tagged: Vec<_> = all()
            .into_iter()
            .filter(|d| d.tags.contains(&tag))
            .collect();
        if tagged.is_empty() {
            return Err(format!("no scenario carries tag `{tag}`"));
        }
        for def in tagged {
            if !selected.iter().any(|d| d.name == def.name) {
                selected.push(def);
            }
        }
    }
    if selected.is_empty() {
        return Err("no scenario selected: pass names or --tag".into());
    }
    Ok(selected)
}

/// Rejects duplicate names loudly at startup.
pub fn validate() -> Result<(), String> {
    let defs = all();
    for pair in defs.windows(2) {
        if pair[0].name == pair[1].name {
            return Err(format!("duplicate scenario name `{}`", pair[0].name));
        }
    }
    Ok(())
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod backend_tests {
    use super::*;

    const TRADE: &[Capability] = &[
        Capability::Onboarding,
        Capability::Nwc,
        Capability::CreateOrder,
        Capability::HoldInvoicePayment,
        Capability::BuyerInvoice,
    ];

    const ALICE_ONLY: ManualPayments = ManualPayments {
        alice: true,
        bob: false,
    };
    const BOB_ONLY: ManualPayments = ManualPayments {
        alice: false,
        bob: true,
    };

    /// The app's own wallet is what the scenario declared; nothing to
    /// resolve.
    #[test]
    fn on_the_apps_own_wallet_the_requirements_are_what_the_scenario_declared() {
        assert_eq!(required_on(TRADE, ManualPayments::NONE), TRADE.to_vec());
    }

    /// On nodes Mortsom holds, the run drives the manual controls and
    /// never touches the app's wallet. Requiring `nwc` would skip a profile
    /// over a capability the run does not use, and not requiring
    /// `external-wallet` would start a profile that cannot pay by hand and
    /// let it fail mid-trade instead of being skipped with a reason.
    #[test]
    fn on_nodes_mortsom_holds_the_manual_controls_replace_the_apps_wallet() {
        let resolved = required_on(TRADE, ManualPayments::BOTH);

        assert!(!resolved.contains(&Capability::Nwc));
        assert!(!resolved.contains(&Capability::HoldInvoicePayment));
        assert!(resolved.contains(&Capability::ExternalWallet));
        assert!(resolved.contains(&Capability::Onboarding));
        assert!(resolved.contains(&Capability::CreateOrder));
    }

    /// `nwc` and `hold-invoice-payment` resolve to the same controls, and
    /// one missing capability must be reported once.
    #[test]
    fn two_wallet_capabilities_resolve_to_one_requirement() {
        let resolved = required_on(TRADE, ManualPayments::BOTH);

        assert_eq!(
            resolved
                .iter()
                .filter(|c| **c == Capability::ExternalWallet)
                .count(),
            1,
            "{resolved:?}"
        );
    }

    /// One actor on a node Mortsom holds and the other on its own wallet
    /// is a configuration `[wallets]` accepts. The half that still pays
    /// through the app reaches `nwc`, so dropping it run-wide would let a
    /// profile without it start and fail mid-trade.
    #[test]
    fn a_run_that_moves_only_the_sellers_wallet_still_needs_the_apps_wallet() {
        let resolved = required_on(TRADE, ALICE_ONLY);

        assert!(
            resolved.contains(&Capability::Nwc),
            "bob still pays through the app: {resolved:?}"
        );
        assert!(!resolved.contains(&Capability::HoldInvoicePayment));
        assert!(resolved.contains(&Capability::ExternalWallet));
    }

    /// Bob's node issues the payout invoice, so the app has to accept one
    /// typed in; Alice still pays the hold invoice through her own wallet.
    #[test]
    fn a_run_that_moves_only_the_buyers_wallet_keeps_the_payment_the_app_makes() {
        let resolved = required_on(TRADE, BOB_ONLY);

        assert!(resolved.contains(&Capability::Nwc));
        assert!(
            resolved.contains(&Capability::HoldInvoicePayment),
            "alice still pays through the app: {resolved:?}"
        );
        assert!(resolved.contains(&Capability::BuyerInvoice));
        assert!(
            resolved.contains(&Capability::ExternalWallet),
            "bob's invoice is ours to hand over: {resolved:?}"
        );
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    fn noop(_ctx: &Context) -> ScenarioFuture<'_> {
        Box::pin(async { Ok(()) })
    }

    inventory::submit! {
        ScenarioDef {
            name: "zz_kit_selftest",
            tags: &["kit-selftest"],
            requires: &[],
            timeout_secs: 1,
            moves_value: false,
            needs_app_wallet: false,
            needs_solver: false,
            needs: &[],
            run: noop,
        }
    }

    #[test]
    fn registry_finds_and_selects() {
        assert!(find("zz_kit_selftest").is_some());
        assert!(find("nope").is_none());
        assert!(select(&["nope".into()], None).is_err());
        assert!(select(&[], Some("no-such-tag")).is_err());
        assert!(select(&[], None).is_err());
        let by_tag = select(&[], Some("kit-selftest")).unwrap_or_default();
        assert_eq!(by_tag.len(), 1);
        assert!(validate().is_ok());
    }
    #[test]
    fn a_name_repeated_on_the_command_line_runs_once() {
        let names = vec!["zz_kit_selftest".to_owned(), "zz_kit_selftest".to_owned()];

        let selected = select(&names, None).unwrap();

        assert_eq!(selected.len(), 1, "a scenario must not run twice");
    }
}
