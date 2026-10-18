//! Which daemon a scenario is written for: the `bond` family for one that
//! asks for anti-abuse bonds (`stack up --bond`), every other family for one
//! that does not. A bonded daemon parks every order and every take on a
//! bond the other families never pay, and a bondless one never opens the
//! pay-bond screen the bond family waits for. A scenario run against the
//! other kind of daemon is skipped with the reason, never failed (spec 8.1).

use std::time::Duration;

use mortsom_scenario_kit::registry::ScenarioDef;

use super::{Outcome, ScenarioResult};

/// The family whose scenarios need a bonded daemon.
pub(super) const BOND_FAMILY: &str = "bond";

/// Splits `selected` into the scenarios written for the stack's daemon and
/// the rest, skipped with the reason. `bonded` is whether the stack declares
/// an anti-abuse bond, `None` without a Mortsom-owned stack: then nothing
/// declares one, and the bond family is skipped while the others run.
pub(super) fn gate_bond<'a>(
    profile: &str,
    selected: Vec<&'a ScenarioDef>,
    bonded: Option<bool>,
) -> (Vec<&'a ScenarioDef>, Vec<ScenarioResult>) {
    let mut runnable = Vec::new();
    let mut skipped = Vec::new();
    for def in selected {
        let wants_bond = def.tags.contains(&BOND_FAMILY);
        let reason = match (wants_bond, bonded) {
            (true, Some(true)) | (false, Some(false) | None) => {
                runnable.push(def);
                continue;
            }
            (true, Some(false)) => format!(
                "{} is written for a daemon that asks for anti-abuse bonds, and this stack's \
                 asks for none (`mortsom stack up --bond`)",
                def.name
            ),
            (true, None) => format!(
                "{} is written for a daemon that asks for anti-abuse bonds, which only a \
                 Mortsom-owned stack declares (`mortsom stack up --bond`)",
                def.name
            ),
            (false, Some(true)) => format!(
                "{} is written for a daemon without anti-abuse bonds, and this stack's asks \
                 every order for one (`mortsom stack up` without `--bond`)",
                def.name
            ),
        };
        skipped.push(ScenarioResult {
            name: def.name.into(),
            app_profile: profile.to_owned(),
            app_version: None,
            outcome: Outcome::Skipped { reason },
            attempts: 0,
            duration: Duration::ZERO,
        });
    }
    (runnable, skipped)
}

#[cfg(test)]
mod tests {
    use super::*;
    use mortsom_scenario_kit::context::Context;
    use mortsom_scenario_kit::registry::ScenarioFuture;

    fn noop(_ctx: &Context) -> ScenarioFuture<'_> {
        Box::pin(async { Ok(()) })
    }

    fn def(name: &'static str, tags: &'static [&'static str]) -> ScenarioDef {
        ScenarioDef {
            name,
            tags,
            requires: &[],
            timeout_secs: 1,
            moves_value: false,
            needs_app_wallet: false,
            needs_solver: false,
            needs: &[],
            run: noop,
        }
    }

    fn names(defs: &[&ScenarioDef]) -> Vec<&'static str> {
        defs.iter().map(|d| d.name).collect()
    }

    fn reason(result: &ScenarioResult) -> &str {
        match &result.outcome {
            Outcome::Skipped { reason } => reason,
            other => panic!("not skipped: {other:?}"),
        }
    }

    #[test]
    fn a_bonded_stack_runs_the_bond_family_and_skips_the_rest() {
        let bonded = def("maker_bond_abandoned", &["bond"]);
        let plain = def("happy_sell", &["happy-path", "smoke"]);

        let (runnable, skipped) = gate_bond("mobile-v2", vec![&bonded, &plain], Some(true));

        assert_eq!(names(&runnable), ["maker_bond_abandoned"]);
        assert_eq!(skipped.len(), 1);
        assert_eq!(skipped[0].name, "happy_sell");
        assert_eq!(skipped[0].attempts, 0);
        assert!(reason(&skipped[0]).contains("without `--bond`"));
    }

    #[test]
    fn a_bondless_stack_skips_the_bond_family_and_runs_the_rest() {
        let bonded = def("maker_bond_abandoned", &["bond"]);
        let plain = def("happy_sell", &["happy-path", "smoke"]);

        let (runnable, skipped) = gate_bond("mobile-v2", vec![&bonded, &plain], Some(false));

        assert_eq!(names(&runnable), ["happy_sell"]);
        assert_eq!(skipped[0].name, "maker_bond_abandoned");
        assert!(reason(&skipped[0]).contains("stack up --bond"));
    }

    /// Without a Mortsom-owned stack nothing declares a bond: the bond
    /// family cannot know what it would be paying, the rest runs as ever.
    #[test]
    fn without_an_owned_stack_only_the_bond_family_is_skipped() {
        let bonded = def("maker_bond_abandoned", &["bond"]);
        let plain = def("happy_sell", &["happy-path"]);

        let (runnable, skipped) = gate_bond("mobile-v2", vec![&bonded, &plain], None);

        assert_eq!(names(&runnable), ["happy_sell"]);
        assert!(reason(&skipped[0]).contains("Mortsom-owned stack"));
    }
}
