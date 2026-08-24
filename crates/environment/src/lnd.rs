//! Direct LND access over gRPC: the wallet backend behind a local
//! [Polar](https://lightningpolar.com/) network.
//!
//! On the NWC path the app holds the wallet and pays for itself; Mortsom
//! only watches. Here there is no wallet in the app at all, so Mortsom is
//! the one that pays the hold invoice and issues the buyer invoice, through
//! the app's manual controls. That is what makes a whole trade settle in a
//! fraction of a second on regtest instead of costing mainnet sats and a
//! wallet service's latency.
//!
//! Nothing in this module logs an invoice, a macaroon or a certificate.

use std::time::Duration;

use fedimint_tonic_lnd::{Client, lnrpc, routerrpc};
use mortsom_common::Secret;
use thiserror::Error;

use crate::lnd_target;

/// Timeout for a single gRPC call.
pub const RPC_TIMEOUT: Duration = Duration::from_secs(30);

/// How long a payment attempt may stay in flight. A Mostro hold invoice can
/// legitimately occupy the whole taker window, so this is not a latency
/// budget — it is the point past which the run has already failed.
pub const PAYMENT_TIMEOUT: Duration = Duration::from_secs(600);

/// Routing-fee ceiling per payment. On regtest the graph is two hops and
/// this never binds; it is the rail that keeps a misconfigured mainnet node
/// from paying an unbounded fee.
pub const MAX_ROUTING_FEE_SATS: i64 = 50;

/// Failures talking to an LND node. Messages carry the endpoint and the
/// operation, never a credential or an invoice.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum LndError {
    /// A configured credential file is not where the config says it is.
    #[error("{label}: LND {field} file not found: {path}")]
    CredentialNotFound {
        /// Wallet label.
        label: String,
        /// `cert` or `macaroon`.
        field: &'static str,
        /// Path looked for, with any `~` already expanded.
        path: String,
    },
    /// The node did not accept the connection.
    #[error("{label}: cannot connect to LND at {endpoint}: {message}")]
    Connect {
        /// Wallet label.
        label: String,
        /// gRPC endpoint tried.
        endpoint: String,
        /// Error text from the gRPC layer.
        message: String,
    },
    /// A call did not answer in time.
    #[error("{label}: LND {operation} timed out after {timeout:?}")]
    Timeout {
        /// Wallet label.
        label: String,
        /// Operation that timed out.
        operation: &'static str,
        /// Deadline used.
        timeout: Duration,
    },
    /// The node returned an error.
    #[error("{label}: LND {operation} failed: {message}")]
    Failed {
        /// Wallet label.
        label: String,
        /// Operation that failed.
        operation: &'static str,
        /// Error text from the node.
        message: String,
    },
    /// A payment ended without a terminal status.
    #[error("{label}: payment {hash} ended with no terminal status")]
    NoTerminalStatus {
        /// Wallet label.
        label: String,
        /// Payment hash, which is public.
        hash: String,
    },
}

/// Terminal or in-flight state of a payment, as LND reports it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LndPaymentState {
    /// Open, or a hold-invoice HTLC locked in and awaiting settlement.
    Pending,
    /// Settled.
    Settled,
    /// Cancelled or definitively failed.
    Failed,
}

/// What a settled outgoing payment cost.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LndPayment {
    /// Payment hash, hex.
    pub hash: String,
    /// Routing fees actually paid, in sats.
    pub fees_paid_sats: u64,
}

/// A connected LND node.
#[derive(Clone)]
pub struct LndWallet {
    label: String,
    /// `Client` wraps a tonic channel and is cheap to clone, so each call
    /// clones it rather than holding a lock: a hold-invoice payment in
    /// flight for minutes never blocks a lookup on the same actor.
    client: Client,
}

impl std::fmt::Debug for LndWallet {
    /// The client holds the macaroon, so it never reaches Debug output.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LndWallet")
            .field("label", &self.label)
            .finish_non_exhaustive()
    }
}

impl LndWallet {
    /// Connects using the configured host, TLS certificate and macaroon,
    /// both of which are filesystem paths.
    pub async fn connect(
        label: &str,
        host: &str,
        cert: &Secret,
        macaroon: &Secret,
    ) -> Result<Self, LndError> {
        let endpoint = lnd_target::grpc_endpoint(host);
        let cert_path = lnd_target::credential_path(cert.expose());
        let macaroon_path = lnd_target::credential_path(macaroon.expose());
        // A missing credential file surfaces from the gRPC layer as an
        // opaque failure, so name it here instead: with Polar, the path is
        // what is wrong nine times out of ten, because the network number
        // changes with every network created.
        for (field, path) in [("cert", &cert_path), ("macaroon", &macaroon_path)] {
            if !path.exists() {
                return Err(LndError::CredentialNotFound {
                    label: label.to_owned(),
                    field,
                    path: path.display().to_string(),
                });
            }
        }
        let client = fedimint_tonic_lnd::connect(
            endpoint.clone(),
            cert_path.to_string_lossy().into_owned(),
            macaroon_path.to_string_lossy().into_owned(),
        )
        .await
        .map_err(|e| LndError::Connect {
            label: label.to_owned(),
            endpoint,
            message: e.to_string(),
        })?;
        Ok(Self {
            label: label.to_owned(),
            client,
        })
    }

    /// Wallet label (`alice`, `bob`).
    pub fn label(&self) -> &str {
        &self.label
    }

    /// Node alias and the network it runs on, for `mortsom doctor`.
    pub async fn info(&self) -> Result<(String, String), LndError> {
        let mut client = self.client.clone();
        let response = self
            .call(
                "get_info",
                client.lightning().get_info(lnrpc::GetInfoRequest {}),
            )
            .await?
            .into_inner();
        let network = response
            .chains
            .first()
            .map(|c| c.network.clone())
            .unwrap_or_default();
        Ok((response.alias, network))
    }

    /// Outbound and inbound channel liquidity in sats, for `mortsom doctor`.
    ///
    /// Both matter: a node with no inbound liquidity cannot be paid, which
    /// on regtest is the usual reason a freshly created Polar network fails
    /// a trade halfway through.
    pub async fn channel_balance(&self) -> Result<(u64, u64), LndError> {
        let mut client = self.client.clone();
        let response = self
            .call(
                "channel_balance",
                client
                    .lightning()
                    .channel_balance(lnrpc::ChannelBalanceRequest {}),
            )
            .await?
            .into_inner();
        let outbound = response.local_balance.map(|a| a.sat).unwrap_or(0);
        let inbound = response.remote_balance.map(|a| a.sat).unwrap_or(0);
        Ok((outbound, inbound))
    }

    /// Creates an invoice to receive `sats`, returning the bolt11.
    ///
    /// Protected because it is typed into the app and ends up on the wire:
    /// an invoice is not a credential, but it is not evidence either.
    pub async fn make_invoice(&self, sats: u64, memo: &str) -> Result<Secret, LndError> {
        let mut client = self.client.clone();
        let response = self
            .call(
                "add_invoice",
                client.lightning().add_invoice(lnrpc::Invoice {
                    memo: memo.to_owned(),
                    value: sats as i64,
                    ..Default::default()
                }),
            )
            .await?
            .into_inner();
        Ok(Secret::new(response.payment_request))
    }

    /// Starts paying `invoice` and returns once the node has the payment in
    /// flight — it does **not** wait for settlement.
    ///
    /// A Mostro hold invoice is accepted and then held: the HTLC stays
    /// locked in until the daemon settles it at release, minutes later and
    /// several scenario steps away. Waiting here would stall the trade that
    /// is supposed to release it, so the stream is drained by a background
    /// task and settlement is confirmed where it always was, by the wallet
    /// observer.
    ///
    /// An immediate failure — no route, unknown invoice — still surfaces
    /// here, because it arrives as the first update.
    pub async fn start_payment(&self, invoice: &Secret) -> Result<String, LndError> {
        let mut client = self.client.clone();
        let mut stream = client
            .router()
            .send_payment_v2(routerrpc::SendPaymentRequest {
                payment_request: invoice.expose().trim().to_owned(),
                timeout_seconds: PAYMENT_TIMEOUT.as_secs() as i32,
                fee_limit_sat: MAX_ROUTING_FEE_SATS,
                ..Default::default()
            })
            .await
            .map_err(|e| self.failed("send_payment_v2", e.to_string()))?
            .into_inner();

        let first = stream
            .message()
            .await
            .map_err(|e| self.failed("payment stream", e.to_string()))?
            .ok_or_else(|| LndError::NoTerminalStatus {
                label: self.label.clone(),
                hash: String::new(),
            })?;
        if lnrpc::payment::PaymentStatus::try_from(first.status)
            == Ok(lnrpc::payment::PaymentStatus::Failed)
        {
            return Err(self.failed(
                "payment",
                format!("failure reason {}", first.failure_reason),
            ));
        }
        let hash = first.payment_hash.clone();
        let label = self.label.clone();
        let logged = hash.clone();
        // Draining the rest keeps the HTLC owned by a live consumer and
        // gives a failed payment somewhere to be reported, rather than
        // dying silently when the stream is dropped.
        tokio::spawn(async move {
            loop {
                match stream.message().await {
                    Ok(Some(update)) => {
                        match lnrpc::payment::PaymentStatus::try_from(update.status) {
                            Ok(lnrpc::payment::PaymentStatus::Succeeded) => {
                                tracing::info!(wallet = %label, hash = %logged, "payment settled");
                                return;
                            }
                            Ok(lnrpc::payment::PaymentStatus::Failed) => {
                                tracing::warn!(
                                    wallet = %label,
                                    hash = %logged,
                                    reason = update.failure_reason,
                                    "payment failed",
                                );
                                return;
                            }
                            _ => continue,
                        }
                    }
                    Ok(None) => return,
                    Err(e) => {
                        tracing::warn!(wallet = %label, hash = %logged, error = %e, "payment stream ended");
                        return;
                    }
                }
            }
        });
        Ok(hash)
    }

    /// Pays `invoice`, resolving only when the payment settles or fails
    /// definitively. For a hold invoice use [`LndWallet::start_payment`].
    pub async fn pay_invoice(&self, invoice: &Secret) -> Result<LndPayment, LndError> {
        let mut client = self.client.clone();
        let mut stream = client
            .router()
            .send_payment_v2(routerrpc::SendPaymentRequest {
                payment_request: invoice.expose().trim().to_owned(),
                timeout_seconds: PAYMENT_TIMEOUT.as_secs() as i32,
                fee_limit_sat: MAX_ROUTING_FEE_SATS,
                ..Default::default()
            })
            .await
            .map_err(|e| self.failed("send_payment_v2", e.to_string()))?
            .into_inner();

        let mut hash = String::new();
        while let Some(update) = stream
            .message()
            .await
            .map_err(|e| self.failed("payment stream", e.to_string()))?
        {
            hash = update.payment_hash.clone();
            match lnrpc::payment::PaymentStatus::try_from(update.status) {
                Ok(lnrpc::payment::PaymentStatus::Succeeded) => {
                    return Ok(LndPayment {
                        hash: update.payment_hash,
                        fees_paid_sats: (update.fee_msat / 1000).max(0) as u64,
                    });
                }
                Ok(lnrpc::payment::PaymentStatus::Failed) => {
                    return Err(self.failed(
                        "payment",
                        format!("failure reason {}", update.failure_reason),
                    ));
                }
                // Initiated / InFlight: keep waiting for a terminal update.
                _ => continue,
            }
        }
        Err(LndError::NoTerminalStatus {
            label: self.label.clone(),
            hash,
        })
    }

    /// Looks up a payment by hash, incoming or outgoing.
    ///
    /// Both directions matter here. A buyer invoice is one this node issued,
    /// so `lookup_invoice` knows it; a hold invoice was issued by the daemon
    /// and only *paid* by this node, and `lookup_invoice` has never heard of
    /// it — that one is an outgoing payment. Asking only about invoices
    /// would report every hold-invoice payment as unknown.
    ///
    /// `Ok(None)` when the node knows the hash in neither direction.
    pub async fn lookup(
        &self,
        payment_hash: &str,
    ) -> Result<Option<(u64, LndPaymentState)>, LndError> {
        if let Some(incoming) = self.lookup_incoming(payment_hash).await? {
            return Ok(Some(incoming));
        }
        self.lookup_outgoing(payment_hash).await
    }

    /// Status of an outgoing payment, from the router's own record.
    async fn lookup_outgoing(
        &self,
        payment_hash: &str,
    ) -> Result<Option<(u64, LndPaymentState)>, LndError> {
        let Some(hash_bytes) = hex_to_bytes(payment_hash) else {
            return Err(self.failed("track_payment_v2", "payment hash is not hex".to_owned()));
        };
        let mut client = self.client.clone();
        let mut stream = match client
            .router()
            .track_payment_v2(routerrpc::TrackPaymentRequest {
                payment_hash: hash_bytes,
                no_inflight_updates: true,
            })
            .await
        {
            Ok(response) => response.into_inner(),
            Err(status) if status.code() == fedimint_tonic_lnd::tonic::Code::NotFound => {
                return Ok(None);
            }
            Err(status) => return Err(self.failed("track_payment_v2", status.to_string())),
        };
        let update = match tokio::time::timeout(RPC_TIMEOUT, stream.message())
            .await
            .map_err(|_| self.timeout("track_payment_v2"))?
        {
            Ok(Some(update)) => update,
            Ok(None) => return Ok(None),
            Err(status) if status.code() == fedimint_tonic_lnd::tonic::Code::NotFound => {
                return Ok(None);
            }
            Err(status) => return Err(self.failed("track_payment_v2", status.to_string())),
        };
        let state = match lnrpc::payment::PaymentStatus::try_from(update.status) {
            Ok(lnrpc::payment::PaymentStatus::Succeeded) => LndPaymentState::Settled,
            Ok(lnrpc::payment::PaymentStatus::Failed) => LndPaymentState::Failed,
            // A hold invoice sits in flight for as long as the daemon holds
            // it, which is most of a trade.
            _ => LndPaymentState::Pending,
        };
        Ok(Some(((update.value_msat / 1000).max(0) as u64, state)))
    }

    async fn lookup_incoming(
        &self,
        payment_hash: &str,
    ) -> Result<Option<(u64, LndPaymentState)>, LndError> {
        let Some(r_hash) = hex_to_bytes(payment_hash) else {
            return Err(self.failed("lookup_invoice", "payment hash is not hex".to_owned()));
        };
        let mut client = self.client.clone();
        let response = match tokio::time::timeout(
            RPC_TIMEOUT,
            client.lightning().lookup_invoice(lnrpc::PaymentHash {
                r_hash,
                ..Default::default()
            }),
        )
        .await
        .map_err(|_| self.timeout("lookup_invoice"))?
        {
            Ok(response) => response.into_inner(),
            Err(status) if status.code() == fedimint_tonic_lnd::tonic::Code::NotFound => {
                return Ok(None);
            }
            Err(status) => return Err(self.failed("lookup_invoice", status.to_string())),
        };
        let state = match lnrpc::invoice::InvoiceState::try_from(response.state) {
            Ok(lnrpc::invoice::InvoiceState::Settled) => LndPaymentState::Settled,
            Ok(lnrpc::invoice::InvoiceState::Canceled) => LndPaymentState::Failed,
            // Open, and Accepted (a hold-invoice HTLC locked in), are both
            // still in flight.
            _ => LndPaymentState::Pending,
        };
        let sats = if response.amt_paid_sat > 0 {
            response.amt_paid_sat
        } else {
            response.value
        };
        Ok(Some((sats.max(0) as u64, state)))
    }

    async fn call<T>(
        &self,
        operation: &'static str,
        future: impl Future<
            Output = Result<
                fedimint_tonic_lnd::tonic::Response<T>,
                fedimint_tonic_lnd::tonic::Status,
            >,
        >,
    ) -> Result<fedimint_tonic_lnd::tonic::Response<T>, LndError> {
        tokio::time::timeout(RPC_TIMEOUT, future)
            .await
            .map_err(|_| self.timeout(operation))?
            .map_err(|e| self.failed(operation, e.to_string()))
    }

    fn timeout(&self, operation: &'static str) -> LndError {
        LndError::Timeout {
            label: self.label.clone(),
            operation,
            timeout: RPC_TIMEOUT,
        }
    }

    fn failed(&self, operation: &'static str, message: String) -> LndError {
        LndError::Failed {
            label: self.label.clone(),
            operation,
            message,
        }
    }
}

/// Decodes a hex string; `None` when it is not valid hex.
fn hex_to_bytes(hex: &str) -> Option<Vec<u8>> {
    if !hex.len().is_multiple_of(2) || hex.is_empty() {
        return None;
    }
    (0..hex.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).ok())
        .collect()
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn decodes_hex_and_rejects_what_is_not() {
        assert_eq!(hex_to_bytes("00ff"), Some(vec![0x00, 0xff]));
        assert_eq!(hex_to_bytes("0f0"), None, "odd length");
        assert_eq!(hex_to_bytes("zz"), None, "not hex");
        assert_eq!(hex_to_bytes(""), None, "empty");
    }

    /// The macaroon lives inside the client, so a wallet that printed
    /// itself would put it in a log or an artifact.
    #[test]
    fn debug_output_names_the_wallet_and_nothing_else() {
        let error = LndError::CredentialNotFound {
            label: "alice".to_owned(),
            field: "macaroon",
            path: "/home/u/.polar/networks/1/admin.macaroon".to_owned(),
        };

        let rendered = error.to_string();
        assert!(rendered.contains("alice"));
        assert!(rendered.contains("macaroon file not found"));
        // The path is the whole point of the message: it is what is wrong.
        assert!(rendered.contains("networks/1"));
    }

    #[test]
    fn a_missing_credential_is_reported_before_any_connection_is_tried() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();

        let error = runtime
            .block_on(LndWallet::connect(
                "alice",
                "127.0.0.1:10001",
                &Secret::new("/nonexistent/tls.cert"),
                &Secret::new("/nonexistent/admin.macaroon"),
            ))
            .unwrap_err();

        assert!(
            matches!(&error, LndError::CredentialNotFound { field: "cert", .. }),
            "{error}"
        );
    }
}
