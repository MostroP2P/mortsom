//! Blaming the stack for a failure it caused.
//!
//! A scenario sees the stack only through the apps, so when the relay or a
//! node goes away mid-scenario it reports whatever the apps did next: a
//! trade that never appeared reads as a product failure. Once a scenario has
//! failed, the run asks whether the stack still answers. If two probes in a
//! row find something that does not, the stack broke under the scenario and
//! the failure is the infrastructure's; what the scenario reported is kept
//! in the message.
//!
//! Only signs of life count. The pre-run preflight also holds the wallets to
//! the run cap and the daemon's info event to a freshness window; a healthy
//! stack stops meeting both in the ordinary course of a run, so neither may
//! turn an app's failure into the stack's.
use mortsom_common::FailureCategory;
use mortsom_common::check::{CheckStatus, DoctorReport};
use mortsom_scenario_kit::outcome::{Outcome, ScenarioResult};
use mortsom_scenario_kit::timeline::{Timeline, TimelineKind};
use tracing::warn;

use crate::config::Config;
use crate::doctor::{NetProbes, PortState};

#[cfg(test)]
#[path = "attribution_tests.rs"]
mod tests;

/// `result`, blamed on the stack when it is a failure and the stack stopped
/// answering. Read-only: the probes create no order and pay nothing.
pub(super) async fn after_failure(
    config: &Config,
    net: Option<&dyn NetProbes>,
    timeline: &Timeline,
    result: ScenarioResult,
) -> ScenarioResult {
    let (Outcome::Failed { category, .. }, Some(net)) = (&result.outcome, net) else {
        return result;
    };
    if *category == FailureCategory::TestInfrastructure {
        return result;
    }
    if liveness(config, net).await.is_healthy() {
        return result;
    }
    // A second look, so one probe that timed out cannot overturn a verdict.
    let after = liveness(config, net).await;
    let outcome = attribute(result.outcome, &after);
    if let Outcome::Failed { message, .. } = &outcome {
        warn!(scenario = %result.name, %message, "failure attributed to the stack");
        timeline.push(
            TimelineKind::Assertion,
            None,
            format!("scenario {}: {}", result.name, message),
            None,
        );
    }
    ScenarioResult { outcome, ..result }
}

/// Whether the stack answers: the relay's port, a connection to each relay
/// the daemon is configured on, and each actor's wallet.
async fn liveness(config: &Config, net: &dyn NetProbes) -> DoctorReport {
    let mut report = DoctorReport::default();
    let port = config.relay.host_port;
    report.push(
        "relay-port",
        pass_if(net.local_port(port).await == PortState::InUse),
        format!("nothing listens on relay port {port}"),
    );
    if let Some(key) = config.mobile.mostro_pubkey() {
        for relay in net.daemon(&config.mostro.relays, key).await.relays {
            let detail = match &relay.error {
                Some(error) => format!("unreachable via {}: {error}", relay.dialled),
                None => format!("connected via {}", relay.dialled),
            };
            let name = format!("relay {}", relay.configured);
            report.push(name, pass_if(relay.error.is_none()), detail);
        }
    }
    for (actor, wallet) in [
        ("alice", &config.wallets.alice),
        ("bob", &config.wallets.bob),
    ] {
        let (status, detail) = match net.wallet(actor, wallet).await {
            Ok(_) => (CheckStatus::Pass, "answers".to_owned()),
            Err(error) => (CheckStatus::Fail, format!("does not answer: {error}")),
        };
        report.push(format!("wallet {actor}"), status, detail);
    }
    report
}

fn pass_if(alive: bool) -> CheckStatus {
    if alive {
        CheckStatus::Pass
    } else {
        CheckStatus::Fail
    }
}

/// `outcome` as reported, unless it is a failure not already the
/// infrastructure's and the probes in `after` found the stack not answering.
pub(super) fn attribute(outcome: Outcome, after: &DoctorReport) -> Outcome {
    let Outcome::Failed { category, message } = outcome else {
        return outcome;
    };
    if category == FailureCategory::TestInfrastructure || after.is_healthy() {
        return Outcome::Failed { category, message };
    }
    let broken = after
        .checks
        .iter()
        .filter(|check| check.status == CheckStatus::Fail)
        .map(|check| format!("{}: {}", check.name, check.detail))
        .collect::<Vec<_>>()
        .join("; ");
    Outcome::Failed {
        category: FailureCategory::TestInfrastructure,
        message: format!(
            "the stack stopped answering during the scenario ({broken}); \
             the scenario reported {category}: {message}"
        ),
    }
}
