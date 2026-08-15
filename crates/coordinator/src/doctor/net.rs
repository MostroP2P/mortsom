//! Network-side checks: relay port, daemon info event, wallet reachability.
//! The boundary is a trait so tests never touch the network.

use std::time::Duration;

use async_trait::async_trait;
use mortsom_common::Secret;
use mortsom_environment::daemon::{DaemonInfo, DaemonProbe, INFO_STALE_AFTER, probe_daemon};
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
#[derive(Debug, Default, Clone, Copy)]
pub struct RealNet;

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
        probe_daemon(relays, pubkey, DAEMON_TIMEOUT).await
    }

    async fn wallet(&self, label: &str, uri: &Secret) -> Result<WalletProbe, WalletProbeError> {
        probe_nwc(label, uri, WALLET_TIMEOUT).await
    }
}

/// Reports whether the configured relay base port is available.
pub async fn check_relay_port(net: &dyn NetProbes, port: u16, report: &mut DoctorReport) {
    match net.local_port(port).await {
        PortState::Free => report.push("relay-port", CheckStatus::Pass, format!("{port} is free")),
        PortState::InUse => report.push(
            "relay-port",
            CheckStatus::Warn,
            format!("{port} is in use; the run will allocate the next free port"),
        ),
    }
}

/// Runs the daemon checks: relay reachability, info event, protocol, LND
/// and the live relay list.
pub async fn check_daemon(
    net: &dyn NetProbes,
    relays: &[String],
    pubkey: PublicKey,
    report: &mut DoctorReport,
) {
    let probe = net.daemon(relays, pubkey).await;
    evaluate_daemon(&probe, relays, report);
}

/// Pure evaluation of a daemon probe.
pub fn evaluate_daemon(probe: &DaemonProbe, expected_relays: &[String], report: &mut DoctorReport) {
    evaluate_daemon_at(probe, expected_relays, now_unix(), report);
}

/// [`evaluate_daemon`] with an injectable clock.
pub fn evaluate_daemon_at(
    probe: &DaemonProbe,
    expected_relays: &[String],
    now_unix: u64,
    report: &mut DoctorReport,
) {
    evaluate_relays(probe, report);
    evaluate_relay_list(probe, expected_relays, report);
    let Some(info) = &probe.info else {
        report.push(
            "mostro-daemon",
            CheckStatus::Fail,
            "no kind-38385 info event from the configured pubkey on the reachable relays",
        );
        return;
    };
    evaluate_info_freshness(info, now_unix, report);
    evaluate_protocol(info, report);
    evaluate_lightning(info, report);
}

fn evaluate_relays(probe: &DaemonProbe, report: &mut DoctorReport) {
    for relay in &probe.relays {
        match &relay.error {
            None => report.push(
                format!("relay {}", relay.configured),
                CheckStatus::Pass,
                format!("connected via {}", relay.dialled),
            ),
            Some(error) => report.push(
                format!("relay {}", relay.configured),
                CheckStatus::Fail,
                format!("unreachable via {}: {error}", relay.dialled),
            ),
        }
    }
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
    report: &mut DoctorReport,
) {
    let name = format!("wallet {label}");
    if uri.is_empty() {
        report.push(name, CheckStatus::Skip, "secret not resolved");
        return;
    }
    match net.wallet(label, uri).await {
        Ok(probe) => report.push(
            name,
            CheckStatus::Pass,
            format!(
                "reachable; alias {}, network {}, {} sats, methods [{}]",
                probe.alias.as_deref().unwrap_or("-"),
                probe.network.as_deref().unwrap_or("-"),
                probe.balance_sats,
                probe.methods.join(", ")
            ),
        ),
        Err(e) => report.push(name, CheckStatus::Fail, e.to_string()),
    }
}
