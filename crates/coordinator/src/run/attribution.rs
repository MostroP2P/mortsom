//! Blaming the stack for a failure it caused.
//!
//! A scenario sees the stack only through the apps, so when the relay, the
//! daemon or a node goes away mid-scenario it reports whatever the apps did
//! next: a trade that never appeared reads as a product failure. Once a
//! scenario has failed, the run asks whether the stack still answers. If two
//! probes in a row find something that does not, the stack broke under the
//! scenario and the failure is the infrastructure's; what the scenario
//! reported is kept in the message.
//!
//! A stack that answers can still refuse: a daemon in maintenance mode
//! rejects every new order and take. A failure while its info event says so
//! is the environment's, the category the preflight gives the same daemon
//! before a run starts.
//!
//! Only signs of life count. The pre-run preflight also holds the wallets to
//! the run cap and the daemon's info event to a freshness window meant for
//! any daemon; a healthy stack stops meeting both in the ordinary course of
//! a run, so neither may turn an app's failure into the stack's.
use std::path::Path;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use mortsom_common::FailureCategory;
use mortsom_common::check::{CheckStatus, DoctorReport};
use mortsom_environment::daemon::DaemonProbe;
use mortsom_environment::daemon_settings::DaemonSettings;
use mortsom_scenario_kit::outcome::{Outcome, ScenarioResult};
use mortsom_scenario_kit::timeline::{Timeline, TimelineKind};
use tracing::warn;

use super::maintenance;
use crate::config::Config;
use crate::doctor::{NetProbes, PortState};

#[cfg(test)]
#[path = "attribution_tests.rs"]
mod tests;

/// How many of its own publish intervals the daemon may go without a new
/// info event before it counts as gone: one missed publish is tolerated.
const SILENT_INTERVALS: u32 = 2;

/// `result`, blamed on the stack when it is a failure and the stack stopped
/// answering, or answered with the daemon in maintenance mode. Read-only:
/// the probes create no order and pay nothing.
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
    let reported = *category;
    let interval = info_interval(&config.mostro.settings_file);
    let mut after = liveness(config, net, &interval).await;
    if !after.report.is_healthy() {
        // A second look, so one probe that timed out cannot overturn a verdict.
        after = liveness(config, net, &interval).await;
    }
    let outcome = attribute(result.outcome, &after);
    // A category other than the one reported is this attribution's verdict
    // and nothing else's.
    if let Outcome::Failed { category, message } = &outcome
        && *category != reported
    {
        warn!(scenario = %result.name, %category, %message, "failure attributed to the stack");
        timeline.push(
            TimelineKind::Assertion,
            None,
            format!("scenario {}: {}", result.name, message),
            None,
        );
    }
    ScenarioResult { outcome, ..result }
}

/// What the probes taken once a scenario has failed found.
#[derive(Debug)]
pub(super) struct Look {
    /// One check per part of the stack that must answer.
    pub(super) report: DoctorReport,
    /// Whether the daemon's info event advertised maintenance mode.
    pub(super) draining: bool,
}

/// How often the daemon says it publishes its info event, from its own
/// settings; why that is unknown otherwise.
fn info_interval(settings_file: &str) -> Result<Duration, String> {
    let settings = DaemonSettings::load(Path::new(settings_file)).map_err(|e| e.to_string())?;
    settings
        .mostro
        .publish_mostro_info_interval
        .map(Duration::from_secs)
        .ok_or_else(|| format!("{settings_file} declares no publish_mostro_info_interval"))
}

/// Whether the stack answers: the relay's port, a connection to each relay
/// the daemon is configured on, the daemon heard on them, and each actor's
/// wallet; and whether the daemon advertises maintenance mode.
async fn liveness(
    config: &Config,
    net: &dyn NetProbes,
    interval: &Result<Duration, String>,
) -> Look {
    let mut report = DoctorReport::default();
    let mut draining = false;
    let port = config.relay.host_port;
    report.push(
        "relay-port",
        pass_if(net.local_port(port).await == PortState::InUse),
        format!("nothing listens on relay port {port}"),
    );
    if let Some(key) = config.mobile.mostro_pubkey() {
        let probe = net.daemon(&config.mostro.relays, key).await;
        for relay in &probe.relays {
            let detail = match &relay.error {
                Some(error) => format!("unreachable via {}: {error}", relay.dialled),
                None => format!("connected via {}", relay.dialled),
            };
            let name = format!("relay {}", relay.configured);
            report.push(name, pass_if(relay.error.is_none()), detail);
        }
        let (status, detail) = daemon_heard(&probe, interval.clone(), now_unix());
        report.push("mostro-daemon", status, detail);
        draining = probe.info.as_ref().is_some_and(maintenance::advertised);
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
    Look { report, draining }
}

/// Whether the daemon was heard lately: a relay keeps serving its last info
/// event after it dies, so the event's age against the daemon's own publish
/// `interval` is what tells a live daemon from a gone one. Not judged when
/// the interval is unknown, or when no relay answered to hear it on (the
/// relay checks speak for that).
pub(super) fn daemon_heard(
    probe: &DaemonProbe,
    interval: Result<Duration, String>,
    now_unix: u64,
) -> (CheckStatus, String) {
    let interval = match interval {
        Ok(interval) => interval,
        Err(why) => return (CheckStatus::Skip, format!("not judged: {why}")),
    };
    if probe.relays.iter().all(|relay| relay.error.is_some()) {
        return (CheckStatus::Skip, "no relay answered to hear it on".into());
    }
    let Some(info) = &probe.info else {
        return (
            CheckStatus::Fail,
            "no info event from it on a reachable relay".into(),
        );
    };
    let age = info.age(now_unix).as_secs();
    let every = interval.as_secs();
    let status = pass_if(info.age(now_unix) <= interval * SILENT_INTERVALS);
    (
        status,
        format!("its last info event is {age}s old; it publishes one every {every}s"),
    )
}

pub(super) fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_secs())
}

fn pass_if(alive: bool) -> CheckStatus {
    if alive {
        CheckStatus::Pass
    } else {
        CheckStatus::Fail
    }
}

/// `outcome` as reported, unless it is a failure the probes in `after`
/// explain. A stack that stopped answering makes it the infrastructure's,
/// unless it already is. A stack that answers with the daemon in
/// maintenance mode makes it the environment's, unless it already is the
/// infrastructure's or the environment's: the daemon was up and refused
/// what the scenario asked of it, the condition the preflight refuses to
/// start in, under the same category.
pub(super) fn attribute(outcome: Outcome, after: &Look) -> Outcome {
    let Outcome::Failed { category, message } = outcome else {
        return outcome;
    };
    if category == FailureCategory::TestInfrastructure {
        return Outcome::Failed { category, message };
    }
    if after.report.is_healthy() {
        if !after.draining || category == FailureCategory::EnvironmentMisconfiguration {
            return Outcome::Failed { category, message };
        }
        return Outcome::Failed {
            category: FailureCategory::EnvironmentMisconfiguration,
            message: format!(
                "{}; the scenario reported {category}: {message}",
                maintenance::blame()
            ),
        };
    }
    let broken = after
        .report
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
