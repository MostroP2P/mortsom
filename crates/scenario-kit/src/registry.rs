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
    /// Body.
    pub run: ScenarioFn,
}

inventory::collect!(ScenarioDef);

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

/// The capabilities a scenario needs from the app on this run's wallet
/// backend.
///
/// A scenario names the journey, not the plumbing: "fund the escrow" is
/// the same step whether the app pays it through its own connected wallet
/// or Mortsom pays it from a node it holds and types the buyer invoice
/// into the app by hand. Which controls that step actually drives is a
/// property of the run, not of the scenario, so scenarios declare the NWC
/// path and this resolves it -- rather than making every scenario carry
/// both and the gate check requirements the run will never exercise.
///
/// On a node Mortsom holds the app has no wallet at all: `nwc` and
/// `hold-invoice-payment` are never reached, and `external-wallet` -- the
/// manual pay and invoice controls -- is what the run depends on instead.
pub fn required_on(requires: &[Capability], manual_payments: bool) -> Vec<Capability> {
    if !manual_payments {
        return requires.to_vec();
    }
    let mut resolved = Vec::with_capacity(requires.len());
    for capability in requires {
        let needed = match capability {
            Capability::Nwc | Capability::HoldInvoicePayment => Capability::ExternalWallet,
            other => *other,
        };
        // Both map onto the same manual controls; asking for them twice
        // would report one missing capability as two.
        if !resolved.contains(&needed) {
            resolved.push(needed);
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

    /// The app's own wallet is what the scenario declared; nothing to
    /// resolve.
    #[test]
    fn on_the_apps_own_wallet_the_requirements_are_what_the_scenario_declared() {
        assert_eq!(required_on(TRADE, false), TRADE.to_vec());
    }

    /// On a node Mortsom holds, the run drives the manual controls and
    /// never touches the app's wallet. Requiring `nwc` would skip a profile
    /// over a capability the run does not use, and not requiring
    /// `external-wallet` would start a profile that cannot pay by hand and
    /// let it fail mid-trade instead of being skipped with a reason.
    #[test]
    fn on_a_node_mortsom_holds_the_manual_controls_replace_the_apps_wallet() {
        let resolved = required_on(TRADE, true);

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
        let resolved = required_on(TRADE, true);

        assert_eq!(
            resolved
                .iter()
                .filter(|c| **c == Capability::ExternalWallet)
                .count(),
            1,
            "{resolved:?}"
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
