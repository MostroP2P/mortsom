//! Interruptions a run can cause: what its client sessions offer
//! (`DesktopFactory::interruptions`; Linux and Web offer none) and never a
//! relay fault, since a run's relay is the stack's, which it neither
//! restarts nor replays into. A scenario that needs anything else is
//! skipped with the reason, never failed (spec 8.1).

use std::time::Duration;

use mortsom_scenario_kit::registry::{Need, ScenarioDef};

use super::{Outcome, ScenarioResult};

/// Splits `selected` into the scenarios whose needs `provided` covers and
/// the rest, skipped with the needs that are missing.
pub(super) fn gate_needs<'a>(
    profile: &str,
    selected: Vec<&'a ScenarioDef>,
    provided: &[Need],
) -> (Vec<&'a ScenarioDef>, Vec<ScenarioResult>) {
    let mut runnable = Vec::new();
    let mut skipped = Vec::new();
    for def in selected {
        let missing: Vec<&str> = def
            .needs
            .iter()
            .filter(|need| !provided.contains(need))
            .map(|need| need.as_str())
            .collect();
        if missing.is_empty() {
            runnable.push(def);
            continue;
        }
        skipped.push(ScenarioResult {
            name: def.name.into(),
            app_profile: profile.to_owned(),
            app_version: None,
            outcome: Outcome::Skipped {
                reason: format!(
                    "{} needs {}, which this run's clients and relay cannot cause",
                    def.name,
                    missing.join(", ")
                ),
            },
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

    fn def(name: &'static str, needs: &'static [Need]) -> ScenarioDef {
        ScenarioDef {
            name,
            tags: &[],
            requires: &[],
            timeout_secs: 1,
            moves_value: false,
            needs_app_wallet: false,
            needs_solver: false,
            needs,
            run: noop,
        }
    }

    #[test]
    fn a_scenario_needing_what_desktop_cannot_cause_is_skipped_with_the_need() {
        let backgrounds = def("backgrounds", &[Need::Background, Need::RelayFaults]);
        let plain = def("plain", &[]);

        let (runnable, skipped) = gate_needs("mobile-v2", vec![&backgrounds, &plain], &[]);

        assert_eq!(
            runnable.iter().map(|d| d.name).collect::<Vec<_>>(),
            ["plain"]
        );
        assert_eq!(skipped.len(), 1);
        assert_eq!(skipped[0].name, "backgrounds");
        assert_eq!(skipped[0].attempts, 0);
        match &skipped[0].outcome {
            Outcome::Skipped { reason } => {
                assert!(reason.contains("background, relay-faults"), "{reason}");
            }
            other => panic!("not skipped: {other:?}"),
        }
    }

    #[test]
    fn a_run_that_provides_the_need_keeps_the_scenario() {
        let rotates = def("rotates", &[Need::Rotation]);

        let (runnable, skipped) = gate_needs("mobile-v2", vec![&rotates], &[Need::Rotation]);

        assert_eq!(runnable.len(), 1);
        assert!(skipped.is_empty());
    }
}
