//! Network-side checks: relay port, daemon info event, wallet reachability.
//! The boundary is a trait so tests never touch the network.

use std::time::Duration;

use async_trait::async_trait;
use mortsom_environment::WalletConfig;
use mortsom_environment::daemon::{DaemonInfo, DaemonProbe, INFO_STALE_AFTER, probe_daemon};
use mortsom_environment::lnd::LndWallet;
use mortsom_environment::relay_url::same_relay;
use mortsom_environment::wallet::{WalletProbe, WalletProbeError, probe_lnd, probe_nwc};
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
    /// Probes a wallet with read-only calls, whichever backend it is.
    async fn wallet(
        &self,
        label: &str,
        wallet: &WalletConfig,
    ) -> Result<WalletProbe, WalletProbeError>;
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

    async fn wallet(
        &self,
        label: &str,
        wallet: &WalletConfig,
    ) -> Result<WalletProbe, WalletProbeError> {
        match wallet {
            WalletConfig::Nwc { uri } => probe_nwc(label, uri, WALLET_TIMEOUT).await,
            WalletConfig::Lnd {
                host,
                macaroon,
                cert,
            } => {
                let node = LndWallet::connect(label, host, cert, macaroon)
                    .await
                    .map_err(|e| WalletProbeError::Backend(e.to_string()))?;
                probe_lnd(&node).await
            }
        }
    }
}

/// Names the first credential a wallet is missing, so the check is skipped
/// with a reason rather than failing against an empty value.
fn unresolved_credential(wallet: &WalletConfig) -> Option<&'static str> {
    match wallet {
        WalletConfig::Nwc { uri } => uri.is_empty().then_some("secret not resolved"),
        WalletConfig::Lnd {
            host,
            macaroon,
            cert,
        } => {
            if host.trim().is_empty() {
                Some("host not resolved")
            } else if macaroon.is_empty() || cert.is_empty() {
                Some("credential paths not resolved")
            } else {
                None
            }
        }
    }
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
) -> Option<DaemonInfo> {
    let probe = net.daemon(relays, pubkey).await;
    evaluate_daemon_with_run_relay(&probe, relays, run_relay, now_unix(), report);
    probe.info
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
        let any_relay_reachable = probe.relays.iter().any(|r| r.error.is_none());
        let daemon_is_publishing = probe.relay_list.is_some() && any_relay_reachable;
        let (status, detail) = if !any_relay_reachable {
            // Nothing was reachable, so nothing was observed: the same
            // verdict the relay list check reaches from the same facts.
            (CheckStatus::Skip, "no reachable relay")
        } else if daemon_is_publishing {
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
    wallet: &WalletConfig,
    caps: Liquidity,
    report: &mut DoctorReport,
) -> Option<WalletProbe> {
    let name = format!("wallet {label}");
    if let Some(unresolved) = unresolved_credential(wallet) {
        report.push(name, CheckStatus::Skip, unresolved);
        return None;
    }
    match net.wallet(label, wallet).await {
        Ok(probe) => {
            // Inbound liquidity is reported where the backend knows it: on
            // a freshly created Polar network a node with none cannot be
            // paid, which is the usual reason a trade dies halfway through.
            let liquidity = match probe.inbound_sats {
                Some(inbound) => format!(
                    "{} sats outbound / {inbound} sats inbound",
                    probe.balance_sats
                ),
                None => format!("{} sats", probe.balance_sats),
            };
            let detail = format!(
                "reachable; {} backend, alias {}, network {}, {liquidity}, methods [{}]",
                wallet.kind(),
                probe.alias.as_deref().unwrap_or("-"),
                probe.network.as_deref().unwrap_or("-"),
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
            } else if probe.inbound_sats.is_some_and(|inbound| inbound < floor) {
                // A node with outbound but no inbound passes every balance
                // check and then cannot receive the buyer payout: the trade
                // dies after the sats already moved. Fail it here instead.
                (
                    CheckStatus::Fail,
                    format!(
                        "{detail}; inbound liquidity below the run cap ({floor} sats): the node cannot receive the payout, open or rebalance a channel towards it"
                    ),
                )
            } else {
                (CheckStatus::Pass, detail)
            };
            report.push(name, status, detail);
            Some(probe)
        }
        Err(e) => {
            report.push(name, CheckStatus::Fail, e.to_string());
            None
        }
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
