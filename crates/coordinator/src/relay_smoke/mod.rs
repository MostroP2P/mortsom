//! `mortsom relay smoke` (spec Phase 1.5): start the run relay, prove the
//! external daemon connects to it and republishes its relay list, collect
//! redacted relay evidence, and always tear the container down. No app, no
//! order, no payment.

use std::panic::AssertUnwindSafe;
use std::time::Duration;

use async_trait::async_trait;
use futures::FutureExt;
use mortsom_common::check::{CheckStatus, DoctorReport};
use mortsom_common::logging::attach_run_log;
use mortsom_common::{Lane, Redactor, RunId};
use mortsom_environment::daemon::{DaemonInfo, RelayList};
use mortsom_environment::docker::ContainerRuntime;
use mortsom_environment::relay::{
    ReadinessProbe, RelayHandle, RelayTeardown, start_relay, teardown_relay,
};
use mortsom_environment::relay_url::same_relay;
use mortsom_observers::relay::{ObservedEvent, ObserverScope, RelayObserver};
use mostro_core::prelude::NOSTR_INFO_EVENT_KIND;
use nostr_sdk::prelude::{Kind, PublicKey};
use serde::{Deserialize, Serialize};
use tracing::{error, info};

use crate::artifacts::{RunArtifacts, files};
use crate::config::Config;
use crate::report::{RunReport, write_report};

/// Relay-side evidence source; the real one wraps [`RelayObserver`].
#[async_trait]
pub trait EvidenceSource: Send + Sync {
    /// Starts observing `relay_url` for the daemon `pubkey`.
    async fn start(
        &self,
        relay_url: &str,
        pubkey: PublicKey,
    ) -> Result<Box<dyn EvidenceSession>, String>;
}

/// A live evidence session.
#[async_trait]
pub trait EvidenceSession: Send + Sync {
    /// Waits for the first event of `kind`, bounded by `timeout`.
    async fn wait_for_kind(&self, kind: u16, timeout: Duration) -> Option<ObservedEvent>;
    /// Everything observed so far, in order.
    fn snapshot(&self) -> Vec<ObservedEvent>;
    /// Stops observing.
    async fn stop(&self);
}

/// The real relay observer.
#[derive(Debug, Default, Clone, Copy)]
pub struct RelayObserverSource;

#[async_trait]
impl EvidenceSource for RelayObserverSource {
    async fn start(
        &self,
        relay_url: &str,
        pubkey: PublicKey,
    ) -> Result<Box<dyn EvidenceSession>, String> {
        let observer = RelayObserver::start(
            &[relay_url.to_owned()],
            ObserverScope::daemon_only(pubkey),
            Duration::from_secs(10),
        )
        .await
        .map_err(|e| e.to_string())?;
        Ok(Box::new(observer))
    }
}

#[async_trait]
impl EvidenceSession for RelayObserver {
    async fn wait_for_kind(&self, kind: u16, timeout: Duration) -> Option<ObservedEvent> {
        self.wait_for(timeout, |e| e.kind == kind).await
    }
    fn snapshot(&self) -> Vec<ObservedEvent> {
        RelayObserver::snapshot(self)
    }
    async fn stop(&self) {
        RelayObserver::stop(self).await;
    }
}

/// Options for the smoke run.
#[derive(Debug, Clone, Default)]
pub struct SmokeOptions {
    /// Keep the relay container and data when the smoke fails, for diagnosis.
    pub keep_on_failure: bool,
}

/// Machine-readable outcome, written to `summary.json`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SmokeSummary {
    /// Run identifier.
    pub run_id: RunId,
    /// Lane.
    pub lane: Lane,
    /// Command name.
    pub command: String,
    /// Relay handle, when the relay started.
    pub relay: Option<RelayHandle>,
    /// Daemon relay list observed on the run relay, if any.
    pub daemon_relay_list: Option<RelayList>,
    /// Daemon info event observed on the run relay, if any.
    pub daemon_info: Option<DaemonInfo>,
    /// Number of relay events recorded.
    pub relay_events: usize,
    /// Checks performed.
    pub checks: DoctorReport,
    /// Teardown result, if a relay was started.
    pub teardown: Option<RelayTeardown>,
    /// True when every required check passed.
    pub passed: bool,
}

/// Runs the smoke. Returns the summary; the caller maps it to an exit code.
pub async fn run_relay_smoke(
    config: &Config,
    options: &SmokeOptions,
    runtime: &dyn ContainerRuntime,
    probe: &dyn ReadinessProbe,
    evidence: &dyn EvidenceSource,
) -> Result<SmokeSummary, String> {
    let run_id = RunId::new();
    let redactor = Redactor::new().with_secrets(config.wallets.secrets());
    let artifacts = RunArtifacts::create(
        std::path::Path::new(&config.run.artifacts_dir),
        run_id.clone(),
        redactor.clone(),
    )
    .map_err(|e| format!("cannot create artifacts: {e}"))?;
    attach_run_log(artifacts.dir(), redactor).map_err(|e| format!("cannot attach run log: {e}"))?;
    info!(run = %run_id, lane = %config.run.lane, "relay smoke starting");

    let mut summary = SmokeSummary {
        run_id: run_id.clone(),
        lane: config.run.lane,
        command: "relay smoke".into(),
        relay: None,
        daemon_relay_list: None,
        daemon_info: None,
        relay_events: 0,
        checks: DoctorReport::default(),
        teardown: None,
        passed: false,
    };
    let Some(pubkey) = config.mobile.mostro_pubkey() else {
        summary
            .checks
            .push("mostro-pubkey", CheckStatus::Fail, "invalid daemon pubkey");
        finish(&artifacts, &mut summary)?;
        return Ok(summary);
    };
    let timeout = Duration::from_secs(config.mostro.health_timeout_secs);

    let handle = match start_relay(
        runtime,
        probe,
        &config.relay,
        run_id.as_str(),
        artifacts.dir(),
        timeout,
    )
    .await
    {
        Ok(handle) => handle,
        Err(e) => {
            summary
                .checks
                .push("relay-start", CheckStatus::Fail, e.to_string());
            finish(&artifacts, &mut summary)?;
            return Ok(summary);
        }
    };
    summary
        .checks
        .push("relay-start", CheckStatus::Pass, handle.host_url());
    summary.relay = Some(handle.clone());

    // A panic while observing must not leak the container: catch it, record
    // it as a failed check and fall through to teardown.
    let observed = AssertUnwindSafe(observe_daemon(
        config,
        pubkey,
        &handle,
        timeout,
        evidence,
        &artifacts,
        &mut summary,
    ))
    .catch_unwind()
    .await;
    if observed.is_err() {
        summary.passed = false;
        summary.checks.push(
            "relay-observer",
            CheckStatus::Fail,
            "observer panicked; see run.log",
        );
    }

    teardown_and_record(runtime, &handle, options, &artifacts, &mut summary).await;
    finish(&artifacts, &mut summary)?;
    Ok(summary)
}

async fn teardown_and_record(
    runtime: &dyn ContainerRuntime,
    handle: &RelayHandle,
    options: &SmokeOptions,
    artifacts: &RunArtifacts,
    summary: &mut SmokeSummary,
) {
    let keep = options.keep_on_failure && !summary.passed;
    let (teardown, logs) = teardown_relay(runtime, handle, keep).await;
    if let Err(e) = artifacts.write_text("relay-container.log", &logs) {
        error!(error = %e, "cannot write relay log");
    }
    let status = if teardown.removed {
        CheckStatus::Pass
    } else {
        CheckStatus::Fail
    };
    let detail = teardown
        .error
        .clone()
        .unwrap_or_else(|| "container removed".into());
    summary.checks.push("relay-teardown", status, detail);
    if keep {
        summary.checks.push(
            "relay-data",
            CheckStatus::Warn,
            "relay data directory kept for diagnosis (--keep-on-failure)",
        );
    }
    summary.teardown = Some(teardown);
    summary.passed = summary.passed && summary.checks.is_healthy();
}

async fn observe_daemon(
    config: &Config,
    pubkey: PublicKey,
    handle: &RelayHandle,
    timeout: Duration,
    evidence: &dyn EvidenceSource,
    artifacts: &RunArtifacts,
    summary: &mut SmokeSummary,
) {
    let session = match evidence.start(&handle.host_url(), pubkey).await {
        Ok(session) => session,
        Err(e) => {
            summary.checks.push("relay-observer", CheckStatus::Fail, e);
            return;
        }
    };
    summary.checks.push(
        "relay-observer",
        CheckStatus::Pass,
        "subscribed to daemon self-description",
    );
    let expected = expected_relay_urls(config, handle);
    summary.daemon_relay_list =
        check_relay_list(session.as_ref(), &expected, timeout, summary).await;
    summary.daemon_info = check_daemon_info(session.as_ref(), summary).await;
    session.stop().await;
    let events = session.snapshot();
    for event in &events {
        if let Err(e) = artifacts.append_jsonl(files::RELAY_EVENTS, event) {
            error!(error = %e, "cannot append relay event");
        }
    }
    summary.relay_events = events.len();
    summary.passed = summary.checks.is_healthy();
}

/// The gate of the smoke (spec §16.3 step 4): the daemon must publish its
/// kind-10002 relay list on the run relay and that list must name it.
async fn check_relay_list(
    session: &dyn EvidenceSession,
    expected: &[String],
    timeout: Duration,
    summary: &mut SmokeSummary,
) -> Option<RelayList> {
    let relay_list = session
        .wait_for_kind(Kind::RelayList.as_u16(), timeout)
        .await
        .and_then(|e| relay_list_from(&e));
    match &relay_list {
        Some(list) if list.advertises_any(expected) => summary.checks.push(
            "daemon-uses-run-relay",
            CheckStatus::Pass,
            format!(
                "kind-10002 published on the run relay lists it ({} relays)",
                list.relays.len()
            ),
        ),
        Some(list) => summary.checks.push(
            "daemon-uses-run-relay",
            CheckStatus::Fail,
            format!(
                "kind-10002 arrived but lists {:?}, none of {:?}",
                list.relays, expected
            ),
        ),
        None => summary.checks.push(
            "daemon-uses-run-relay",
            CheckStatus::Fail,
            format!("no kind-10002 from the daemon on the run relay within {timeout:?}"),
        ),
    }
    relay_list
}

/// Best-effort evidence, not a gate: the daemon republishes kind-38385 only
/// every `publish_mostro_info_interval` (five minutes by default), so its
/// absence on a fresh relay is expected and only warns.
async fn check_daemon_info(
    session: &dyn EvidenceSession,
    summary: &mut SmokeSummary,
) -> Option<DaemonInfo> {
    let info = session
        .wait_for_kind(NOSTR_INFO_EVENT_KIND, Duration::from_secs(1))
        .await
        .and_then(|e| info_from(&e));
    match &info {
        Some(info) => summary.checks.push(
            "daemon-info",
            CheckStatus::Pass,
            format!("kind-38385 seen (protocol {:?})", info.protocol_version()),
        ),
        None => summary.checks.push(
            "daemon-info",
            CheckStatus::Warn,
            "kind-38385 not yet republished on the run relay (best-effort evidence, not a gate)",
        ),
    }
    info
}

/// URLs under which the run relay may legitimately appear in the daemon's
/// relay list: the configured daemon-view URLs plus this run's own views.
/// The configured list usually already names the run relay under one of
/// those views, so entries denoting the same endpoint are collapsed: this
/// list is printed when the check fails and a repeated relay reads as a
/// different one.
fn expected_relay_urls(config: &Config, handle: &RelayHandle) -> Vec<String> {
    let mut urls: Vec<String> = Vec::new();
    for url in config
        .mostro
        .relays
        .iter()
        .cloned()
        .chain([handle.host_url(), handle.docker_url()])
    {
        if !urls.iter().any(|kept| same_relay(kept, &url)) {
            urls.push(url);
        }
    }
    urls
}

fn relay_list_from(event: &ObservedEvent) -> Option<RelayList> {
    (event.kind == Kind::RelayList.as_u16()).then(|| RelayList {
        created_at: event.created_at,
        relays: event
            .tags
            .iter()
            .filter(|t| t.first().map(String::as_str) == Some("r"))
            .filter_map(|t| t.get(1))
            .map(|u| u.trim_end_matches('/').to_owned())
            .collect(),
    })
}

fn info_from(event: &ObservedEvent) -> Option<DaemonInfo> {
    (event.kind == NOSTR_INFO_EVENT_KIND).then(|| DaemonInfo {
        pubkey: event.author.clone(),
        created_at: event.created_at,
        tags: event
            .tags
            .iter()
            .filter_map(|t| Some((t.first()?.clone(), t.get(1)?.clone())))
            .collect(),
    })
}

fn finish(artifacts: &RunArtifacts, summary: &mut SmokeSummary) -> Result<(), String> {
    // Host paths stay out of uploadable artifacts: only the container name,
    // port and image describe the relay here.
    let environment = serde_json::json!({
        "run_id": summary.run_id,
        "lane": summary.lane,
        "command": summary.command,
        "relay": summary.relay.as_ref().map(|r| serde_json::json!({
            "container_name": r.container_name,
            "host_port": r.host_port,
            "image": r.image,
        })),
    });
    let report = RunReport {
        stack: None,
        app_artifact: None,
        app_build: None,
        run_id: &summary.run_id,
        lane: summary.lane,
        command: &summary.command,
        app_profile: None,
        checks: &summary.checks,
        passed: summary.passed,
    };
    write_report(
        artifacts,
        &report,
        summary,
        &environment,
        &summary.teardown,
        true,
    )
}

#[cfg(test)]
mod tests;
