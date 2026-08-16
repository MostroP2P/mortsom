//! Network-side checks: relay port, daemon info event, wallet reachability.
//! The boundary is a trait so tests never touch the network.

use std::time::Duration;

use async_trait::async_trait;
use mortsom_common::Secret;
use mortsom_environment::daemon::{DaemonInfo, DaemonProbe, INFO_STALE_AFTER, probe_daemon};
use mortsom_environment::relay_url::same_relay;
use mortsom_environment::wallet::{WalletProbe, WalletProbeError, probe_nwc};
use nostr_sdk::prelude::PublicKey;

use mortsom_common::check::{CheckStatus, DoctorReport};

/// Deadline for the daemon info fetch.
pub const DAEMON_TIMEOUT: Duration = Duration::from_secs(10);
/// Deadline for each wallet call.
pub const WALLET_TIMEOUT: Duration = Duration::from_secs(20);
/// Protocol version Mortsom supports.
pub const SUPPORTED_PROTOCOL_VERSION: u64 = 2;

/// State of a local TCP port.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PortState {
    /// Nothing is listening.
    Free,
    /// Something accepted the connection.
    InUse,
}

/// Network probes used by the doctor.
#[async_trait]
pub trait NetProbes: Send + Sync {
    /// Whether something listens on `127.0.0.1:port`.
    async fn local_port(&self, port: u16) -> PortState;
    /// Connects to the daemon relays and fetches its info event.
    async fn daemon(&self, relays: &[String], pubkey: PublicKey) -> DaemonProbe;
    /// Probes a wallet with read-only calls.
    async fn wallet(&self, label: &str, uri: &Secret) -> Result<WalletProbe, WalletProbeError>;
}

/// The real network.
#[derive(Debug, Clone, Copy)]
pub struct RealNet {
    daemon_timeout: Duration,
}

impl Default for RealNet {
    fn default() -> Self {
        Self {
            daemon_timeout: DAEMON_TIMEOUT,
        }
    }
}

impl RealNet {
    /// Probes the daemon with `timeout` instead of the default. The events
    /// this waits for are republished on the daemon's own cycle, so the
    /// deadline belongs to configuration (`mostro.health_timeout_secs`).
    pub fn with_daemon_timeout(daemon_timeout: Duration) -> Self {
        Self { daemon_timeout }
    }

    /// Deadline used for the daemon probe.
    pub fn daemon_timeout(&self) -> Duration {
        self.daemon_timeout
    }
}

#[async_trait]
impl NetProbes for RealNet {
    async fn local_port(&self, port: u16) -> PortState {
        let connect = tokio::net::TcpStream::connect(("127.0.0.1", port));
        match tokio::time::timeout(Duration::from_secs(2), connect).await {
            Ok(Ok(_)) => PortState::InUse,
            _ => PortState::Free,
        }
    }

    async fn daemon(&self, relays: &[String], pubkey: PublicKey) -> DaemonProbe {
        probe_daemon(relays, pubkey, self.daemon_timeout).await
    }

    async fn wallet(&self, label: &str, uri: &Secret) -> Result<WalletProbe, WalletProbeError> {
        probe_nwc(label, uri, WALLET_TIMEOUT).await
    }
}

/// Reports whether the configured relay base port is available.
pub async fn check_relay_port(
    net: &dyn NetProbes,
    port: u16,
    report: &mut DoctorReport,
) -> PortState {
    let state = net.local_port(port).await;
    match state {
        PortState::Free => report.push("relay-port", CheckStatus::Pass, format!("{port} is free")),
        PortState::InUse => report.push(
            "relay-port",
            CheckStatus::Warn,
            format!("{port} is in use; the run will allocate the next free port"),
        ),
    }
    state
}

/// Mortsom's own relay, as the doctor needs to reason about it: a relay
/// that only exists while a run is live.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunRelay {
    /// URL the run relay is published under on this host.
    pub url: String,
    /// Whether something currently listens on its port.
    pub port_in_use: bool,
}

/// Runs the daemon checks: relay reachability, info event, protocol, LND
/// and the live relay list.
pub async fn check_daemon(
    net: &dyn NetProbes,
    relays: &[String],
    pubkey: PublicKey,
    run_relay: Option<&RunRelay>,
    report: &mut DoctorReport,
) {
    let probe = net.daemon(relays, pubkey).await;
    evaluate_daemon_with_run_relay(&probe, relays, run_relay, now_unix(), report);
}

/// Pure evaluation of a daemon probe, with an injectable clock and the
/// identity of Mortsom's own relay.
pub fn evaluate_daemon_with_run_relay(
    probe: &DaemonProbe,
    expected_relays: &[String],
    run_relay: Option<&RunRelay>,
    now_unix: u64,
    report: &mut DoctorReport,
) {
    evaluate_relays(probe, run_relay, report);
    evaluate_relay_list(probe, expected_relays, report);
    let Some(info) = &probe.info else {
        // The info event is republished on the daemon's own interval, which
        // is long by default. A fresh relay list from the same pubkey proves
        // the daemon is alive and connected, so the missing info event is
        // evidence not yet collected rather than a broken daemon.
        let daemon_is_publishing =
            probe.relay_list.is_some() && probe.relays.iter().any(|r| r.error.is_none());
        let (status, detail) = if daemon_is_publishing {
            (
                CheckStatus::Warn,
                "no kind-38385 info event yet; the daemon is publishing (its kind-10002 arrived), \
                 so it republishes the info event on its own interval",
            )
        } else {
            (
                CheckStatus::Fail,
                "no kind-38385 info event from the configured pubkey on the reachable relays",
            )
        };
        report.push("mostro-daemon", status, detail);
        return;
    };
    evaluate_info_freshness(info, now_unix, report);
    evaluate_protocol(info, report);
    evaluate_lightning(info, report);
}

fn evaluate_relays(probe: &DaemonProbe, run_relay: Option<&RunRelay>, report: &mut DoctorReport) {
    for relay in &probe.relays {
        let name = format!("relay {}", relay.configured);
        match &relay.error {
            None => report.push(
                name,
                CheckStatus::Pass,
                format!("connected via {}", relay.dialled),
            ),
            // Mortsom creates and destroys its relay per run, so between
            // runs its absence is the normal state and says nothing about
            // the setup. It is still a failure when something holds the
            // port, because then the daemon is talking to a stranger.
            Some(error) if is_idle_run_relay(relay, run_relay) => report.push(
                name,
                CheckStatus::Skip,
                format!(
                    "the run relay only exists while a run is live; \
                     `mortsom relay smoke` proves it ({error})"
                ),
            ),
            Some(error) => report.push(
                name,
                CheckStatus::Fail,
                format!("unreachable via {}: {error}", relay.dialled),
            ),
        }
    }
}

/// True when this unreachable relay is Mortsom's own and nothing is
/// listening on its port.
fn is_idle_run_relay(
    relay: &mortsom_environment::daemon::RelayProbe,
    run_relay: Option<&RunRelay>,
) -> bool {
    run_relay.is_some_and(|run| {
        !run.port_in_use
            && (same_relay(&relay.configured, &run.url) || same_relay(&relay.dialled, &run.url))
    })
}

fn evaluate_relay_list(probe: &DaemonProbe, expected_relays: &[String], report: &mut DoctorReport) {
    match &probe.relay_list {
        Some(list) if list.advertises_any(expected_relays) => report.push(
            "mostro-relay-list",
            CheckStatus::Pass,
            format!(
                "kind-10002 advertises the Mortsom relay ({} relays)",
                list.relays.len()
            ),
        ),
        Some(list) => report.push(
            "mostro-relay-list",
            CheckStatus::Fail,
            format!(
                "kind-10002 lists {:?} but none of {:?}",
                list.relays, expected_relays
            ),
        ),
        None if probe.relays.iter().any(|r| r.error.is_none()) => report.push(
            "mostro-relay-list",
            CheckStatus::Fail,
            "no kind-10002 relay list from the daemon on the reachable relays",
        ),
        None => report.push("mostro-relay-list", CheckStatus::Skip, "no reachable relay"),
    }
}

fn evaluate_info_freshness(info: &DaemonInfo, now_unix: u64, report: &mut DoctorReport) {
    let summary = format!(
        "version {} (protocol {}), published {}s ago",
        info.tag("mostro_version").unwrap_or("unknown"),
        info.tag("protocol_version").unwrap_or("unknown"),
        info.age(now_unix).as_secs()
    );
    if info.is_stale(now_unix) {
        report.push(
            "mostro-daemon",
            CheckStatus::Warn,
            format!("{summary}; older than {INFO_STALE_AFTER:?}, the daemon may be down"),
        );
    } else {
        report.push("mostro-daemon", CheckStatus::Pass, summary);
    }
}

fn evaluate_protocol(info: &DaemonInfo, report: &mut DoctorReport) {
    match info.protocol_version() {
        Some(SUPPORTED_PROTOCOL_VERSION) => {
            report.push("mostro-protocol", CheckStatus::Pass, "version 2")
        }
        other => report.push(
            "mostro-protocol",
            CheckStatus::Fail,
            format!("daemon advertises protocol {other:?}; Mortsom requires 2"),
        ),
    }
}

fn evaluate_lightning(info: &DaemonInfo, report: &mut DoctorReport) {
    if info.has_lightning_node() {
        report.push(
            "mostro-lightning",
            CheckStatus::Pass,
            format!(
                "lnd {} alias {}",
                info.tag("lnd_version").unwrap_or("?"),
                info.tag("lnd_node_alias").unwrap_or("?")
            ),
        );
    } else {
        report.push(
            "mostro-lightning",
            CheckStatus::Fail,
            "info event does not report a connected Lightning node",
        );
    }
}

fn now_unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Probes one wallet and reports only redacted metadata.
pub async fn check_wallet(
    net: &dyn NetProbes,
    label: &str,
    uri: &Secret,
    caps: Liquidity,
    report: &mut DoctorReport,
) {
    let name = format!("wallet {label}");
    if uri.is_empty() {
        report.push(name, CheckStatus::Skip, "secret not resolved");
        return;
    }
    match net.wallet(label, uri).await {
        Ok(probe) => {
            let detail = format!(
                "reachable; alias {}, network {}, {} sats, methods [{}]",
                probe.alias.as_deref().unwrap_or("-"),
                probe.network.as_deref().unwrap_or("-"),
                probe.balance_sats,
                probe.methods.join(", ")
            );
            // Pre-flight liquidity (spec §5): a wallet that cannot cover
            // the run cap fails the check before any sats move.
            let floor = caps.max_run_spend_sats.max(caps.max_order_sats);
            let (status, detail) = if probe.balance_sats < floor {
                (
                    CheckStatus::Fail,
                    format!(
                        "{detail}; below the run cap ({floor} sats): top up the dedicated wallet, do not lower the cap"
                    ),
                )
            } else {
                (CheckStatus::Pass, detail)
            };
            report.push(name, status, detail);
        }
        Err(e) => report.push(name, CheckStatus::Fail, e.to_string()),
    }
}

/// The spend caps a wallet must be able to cover.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Liquidity {
    /// Per-order cap.
    pub max_order_sats: u64,
    /// Per-run cap.
    pub max_run_spend_sats: u64,
}
