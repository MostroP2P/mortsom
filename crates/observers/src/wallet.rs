//! Wallet observer (spec §4.3, §16.2): read-only NWC lookups that record
//! payment hash, amount, fee and terminal state for payments that belong to
//! the scenario. It never pays, never creates invoices, and never relies on
//! a global balance. A payment is associated with an order through the
//! invoice the app displayed and its hash.

use std::str::FromStr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use lightning_invoice::Bolt11Invoice;
use mortsom_common::Secret;
use mortsom_environment::lnd::{LndPaymentState, LndWallet};
use nostr_sdk::prelude::{LookupInvoiceRequest, NostrWalletConnectUri, TransactionState};
use nwc::NostrWalletConnect;
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Terminal (or not yet terminal) state of an observed payment.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PaymentState {
    /// Not settled yet.
    Pending,
    /// Settled.
    Settled,
    /// Failed or expired.
    Failed,
    /// The wallet answered without a state.
    Unknown,
}

/// One observation of a payment, safe for artifacts (no invoice text).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PaymentObservation {
    /// Wallet label (`alice`, `bob`).
    pub wallet: String,
    /// Order the payment belongs to.
    pub order_id: String,
    /// Payment hash (hex).
    pub payment_hash: String,
    /// Amount in sats.
    pub amount_sats: u64,
    /// Fee in sats.
    pub fee_sats: u64,
    /// State.
    pub state: PaymentState,
    /// Observation time (unix seconds).
    pub observed_at: u64,
}

/// Wallet observer failures.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum WalletObserverError {
    /// The invoice could not be parsed.
    #[error("invoice is not a valid bolt11 invoice")]
    InvalidInvoice,
    /// The wallet URI is invalid.
    #[error("{label}: NWC URI is invalid")]
    InvalidUri {
        /// Wallet label.
        label: String,
    },
    /// No reader is registered under that label.
    #[error("no wallet is registered as `{label}`")]
    UnknownWallet {
        /// Wallet label asked for.
        label: String,
    },
    /// The wallet did not answer or refused.
    #[error("{label}: lookup of {payment_hash} failed: {message}")]
    Lookup {
        /// Wallet label.
        label: String,
        /// Payment hash.
        payment_hash: String,
        /// Detail (never the URI).
        message: String,
    },
    /// No terminal state within the deadline.
    #[error("{label}: payment {payment_hash} not settled within {timeout:?} (last state {last:?})")]
    NotSettled {
        /// Wallet label.
        label: String,
        /// Payment hash.
        payment_hash: String,
        /// Deadline used.
        timeout: Duration,
        /// Last observed state.
        last: PaymentState,
    },
}

/// Payment hash (hex) of a bolt11 invoice. The invoice stays secret.
pub fn payment_hash_of(invoice: &Secret) -> Result<String, WalletObserverError> {
    let parsed = Bolt11Invoice::from_str(invoice.expose().trim())
        .map_err(|_| WalletObserverError::InvalidInvoice)?;
    Ok(format!("{}", parsed.payment_hash()))
}

/// Amount in sats encoded in a bolt11 invoice, if any.
pub fn amount_sats_of(invoice: &Secret) -> Result<Option<u64>, WalletObserverError> {
    let parsed = Bolt11Invoice::from_str(invoice.expose().trim())
        .map_err(|_| WalletObserverError::InvalidInvoice)?;
    Ok(parsed.amount_milli_satoshis().map(|msat| msat / 1000))
}

/// Exact millisatoshi amount encoded in a BOLT11 invoice, without rounding.
pub fn amount_msats_of(invoice: &Secret) -> Result<Option<u64>, WalletObserverError> {
    let parsed = Bolt11Invoice::from_str(invoice.expose().trim())
        .map_err(|_| WalletObserverError::InvalidInvoice)?;
    Ok(parsed.amount_milli_satoshis())
}

/// Read-only wallet lookups; the real implementation speaks NWC.
#[async_trait]
pub trait WalletReader: Send + Sync {
    /// Looks up a payment by hash. `Ok(None)` when the wallet does not know it.
    async fn lookup(&self, payment_hash: &str) -> Result<Option<(u64, u64, PaymentState)>, String>;
}

/// NWC-backed reader.
pub struct NwcReader {
    wallet: NostrWalletConnect,
    timeout: Duration,
}

impl NwcReader {
    /// Connects a reader for `uri`.
    pub fn new(label: &str, uri: &Secret, timeout: Duration) -> Result<Self, WalletObserverError> {
        let parsed = NostrWalletConnectUri::from_str(uri.expose()).map_err(|_| {
            WalletObserverError::InvalidUri {
                label: label.to_owned(),
            }
        })?;
        Ok(Self {
            wallet: NostrWalletConnect::builder(parsed).timeout(timeout).build(),
            timeout,
        })
    }
}

#[async_trait]
impl WalletReader for NwcReader {
    async fn lookup(&self, payment_hash: &str) -> Result<Option<(u64, u64, PaymentState)>, String> {
        let request = LookupInvoiceRequest {
            payment_hash: Some(payment_hash.to_owned()),
            invoice: None,
        };
        let response = tokio::time::timeout(self.timeout, self.wallet.lookup_invoice(request))
            .await
            .map_err(|_| "lookup timed out".to_owned())?;
        match response {
            Ok(r) => Ok(Some((
                sats_from_msats(r.amount),
                sats_from_msats(r.fees_paid),
                map_state(r.state),
            ))),
            Err(e) => {
                // The NIP-47 error code would be the thing to match, but the
                // wallet client returns an opaque error whose kinds do not
                // carry it; the code and its prose are both matched here.
                let text = e.to_string();
                let lowered = text.to_ascii_lowercase();
                if lowered.contains("not_found") || lowered.contains("not found") {
                    Ok(None)
                } else {
                    Err(text)
                }
            }
        }
    }
}

/// Sats from millisats, rounded up. Observations feed the spend accounting,
/// where a truncated fraction would understate what actually left the
/// wallet; a sub-satoshi remainder counts as a whole satoshi spent.
fn sats_from_msats(msats: u64) -> u64 {
    msats.div_ceil(1000)
}

fn map_state(state: Option<TransactionState>) -> PaymentState {
    match state {
        Some(TransactionState::Settled) => PaymentState::Settled,
        Some(TransactionState::Pending) | Some(TransactionState::Accepted) => PaymentState::Pending,
        Some(TransactionState::Failed) | Some(TransactionState::Expired) => PaymentState::Failed,
        None => PaymentState::Unknown,
    }
}

/// LND-backed reader.
///
/// A node only knows the invoices it issued, so `Ok(None)` here means the
/// counterparty issued this one — the same distinction `NwcReader` draws
/// from a `not_found` error.
pub struct LndReader {
    wallet: LndWallet,
}

impl LndReader {
    /// Wraps a connected node.
    pub fn new(wallet: LndWallet) -> Self {
        Self { wallet }
    }
}

#[async_trait]
impl WalletReader for LndReader {
    async fn lookup(&self, payment_hash: &str) -> Result<Option<(u64, u64, PaymentState)>, String> {
        match self.wallet.lookup(payment_hash).await {
            // The node reports the fee itself: zero for an invoice it
            // issued (the payer's cost, not this node's), and the routing
            // fee actually paid for an outgoing payment such as the hold
            // invoice — which the spend accounting must count.
            Ok(Some((sats, fee, state))) => Ok(Some((sats, fee, map_lnd_state(state)))),
            Ok(None) => Ok(None),
            Err(e) => Err(e.to_string()),
        }
    }
}

fn map_lnd_state(state: LndPaymentState) -> PaymentState {
    match state {
        LndPaymentState::Settled => PaymentState::Settled,
        LndPaymentState::Pending => PaymentState::Pending,
        LndPaymentState::Failed => PaymentState::Failed,
    }
}

/// Records payment observations for the run's dedicated wallets.
pub struct WalletObserver {
    readers: Vec<(String, Arc<dyn WalletReader>)>,
    log: Mutex<Vec<PaymentObservation>>,
    poll: Duration,
}

impl WalletObserver {
    /// Creates an observer over labelled readers.
    pub fn new(readers: Vec<(String, Arc<dyn WalletReader>)>) -> Self {
        Self {
            readers,
            log: Mutex::new(Vec::new()),
            poll: Duration::from_secs(3),
        }
    }

    /// Overrides the polling interval (tests).
    pub fn with_poll(mut self, poll: Duration) -> Self {
        self.poll = poll;
        self
    }

    fn reader(&self, label: &str) -> Result<Arc<dyn WalletReader>, WalletObserverError> {
        self.readers
            .iter()
            .find(|(l, _)| l == label)
            .map(|(_, r)| Arc::clone(r))
            // A label with no reader is a wiring mistake in the run, not a
            // malformed URI; saying so points at the right place.
            .ok_or_else(|| WalletObserverError::UnknownWallet {
                label: label.to_owned(),
            })
    }

    /// Looks up `payment_hash` once on `label`'s wallet and records it.
    pub async fn observe(
        &self,
        label: &str,
        order_id: &str,
        payment_hash: &str,
    ) -> Result<PaymentObservation, WalletObserverError> {
        let reader = self.reader(label)?;
        let found =
            reader
                .lookup(payment_hash)
                .await
                .map_err(|message| WalletObserverError::Lookup {
                    label: label.to_owned(),
                    payment_hash: payment_hash.to_owned(),
                    message,
                })?;
        let (amount_sats, fee_sats, state) = found.unwrap_or((0, 0, PaymentState::Unknown));
        let observation = PaymentObservation {
            wallet: label.to_owned(),
            order_id: order_id.to_owned(),
            payment_hash: payment_hash.to_owned(),
            amount_sats,
            fee_sats,
            state,
            observed_at: now_unix(),
        };
        lock(&self.log).push(observation.clone());
        Ok(observation)
    }

    /// Polls until the payment settles or fails, bounded by `timeout`.
    pub async fn wait_terminal(
        &self,
        label: &str,
        order_id: &str,
        payment_hash: &str,
        timeout: Duration,
    ) -> Result<PaymentObservation, WalletObserverError> {
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            let observation = self.observe(label, order_id, payment_hash).await?;
            if matches!(
                observation.state,
                PaymentState::Settled | PaymentState::Failed
            ) {
                return Ok(observation);
            }
            if tokio::time::Instant::now() >= deadline {
                return Err(WalletObserverError::NotSettled {
                    label: label.to_owned(),
                    payment_hash: payment_hash.to_owned(),
                    timeout,
                    last: observation.state,
                });
            }
            tokio::time::sleep(self.poll).await;
        }
    }

    /// Polls until the wallet knows the payment at all — pending or
    /// terminal — bounded by `timeout`.
    ///
    /// This is the wait the hold-invoice path needs: a Mostro hold invoice
    /// stays pending from funding until release, so the funding step can
    /// only confirm the payment is in flight, never that it settled.
    pub async fn wait_known(
        &self,
        label: &str,
        order_id: &str,
        payment_hash: &str,
        timeout: Duration,
    ) -> Result<PaymentObservation, WalletObserverError> {
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            let observation = self.observe(label, order_id, payment_hash).await?;
            if observation.state != PaymentState::Unknown {
                return Ok(observation);
            }
            if tokio::time::Instant::now() >= deadline {
                return Err(WalletObserverError::NotSettled {
                    label: label.to_owned(),
                    payment_hash: payment_hash.to_owned(),
                    timeout,
                    last: observation.state,
                });
            }
            tokio::time::sleep(self.poll).await;
        }
    }

    /// Everything observed so far.
    pub fn snapshot(&self) -> Vec<PaymentObservation> {
        lock(&self.log).clone()
    }
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|p| p.into_inner())
}

fn now_unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    /// A well-known bolt11 test invoice from BOLT 11 (1 msat? no: 2500 uBTC).
    const INVOICE: &str = "lnbc2500u1pvjluezsp5zyg3zyg3zyg3zyg3zyg3zyg3zyg3zyg3zyg3zyg3zyg3zyg3zygspp5qqqsyqcyq5rqwzqfqqqsyqcyq5rqwzqfqqqsyqcyq5rqwzqfqypqdq5xysxxatsyp3k7enxv4jsxqzpu9qrsgquk0rl77nj30yxdy8j9vdx85fkpmdla2087ne0xh8nhedh8w27kyke0lp53ut353s06fv3qfegext0eh0ymjpf39tuven09sam30g4vgpfna3rh";

    #[test]
    fn payment_hash_and_amount_come_from_the_invoice() {
        let secret = Secret::new(INVOICE);
        let hash = payment_hash_of(&secret).unwrap();
        assert_eq!(
            hash,
            "0001020304050607080900010203040506070809000102030405060708090102"
        );
        assert_eq!(amount_sats_of(&secret).unwrap(), Some(250_000));
        assert!(payment_hash_of(&Secret::new("garbage")).is_err());
    }

    #[test]
    fn exact_invoice_amount_does_not_round_sub_satoshi_values() {
        let invoice = Secret::new(
            "lnbc100p1psj9jhxdqud3jxktt5w46x7unfv9kz6mn0v3jsnp4q0d3p2sfluzdx45tqcsh2pu5qc7lgq0xs578ngs6s0s68ua4h7cvspp5q6rmq35js88zp5dvwrv9m459tnk2zunwj5jalqtyxqulh0l5gflssp5nf55ny5gcrfl30xuhzj3nphgj27rstekmr9fw3ny5989s300gyus9qyysgqcqpcrzjqw2sxwe993h5pcm4dxzpvttgza8zhkqxpgffcrf5v25nwpr3cmfg7z54kuqq8rgqqqqqqqq2qqqqq9qq9qrzjqd0ylaqclj9424x9m8h2vcukcgnm6s56xfgu3j78zyqzhgs4hlpzvznlugqq9vsqqqqqqqlgqqqqqeqq9qrzjqwldmj9dha74df76zhx6l9we0vjdquygcdt3kssupehe64g6yyp5yz5rhuqqwccqqyqqqqlgqqqqjcqq9qrzjqf9e58aguqr0rcun0ajlvmzq3ek63cw2w282gv3z5uupmuwvgjtq2z55qsqqg6qqqyqqqrtnqqqzq3cqygrzjqvphmsywntrrhqjcraumvc4y6r8v4z5v593trte429v4hredj7ms5z52usqq9ngqqqqqqqlgqqqqqqgq9qrzjq2v0vp62g49p7569ev48cmulecsxe59lvaw3wlxm7r982zxa9zzj7z5l0cqqxusqqyqqqqlgqqqqqzsqygarl9fh38s0gyuxjjgux34w75dnc6xp2l35j7es3jd4ugt3lu0xzre26yg5m7ke54n2d5sym4xcmxtl8238xxvw5h5h5j5r6drg6k6zcqj0fcwg",
        );
        assert_eq!(amount_sats_of(&invoice).unwrap(), Some(0));
        assert_eq!(amount_msats_of(&invoice).unwrap(), Some(10));
    }

    struct Scripted(Mutex<Vec<Option<(u64, u64, PaymentState)>>>);

    #[async_trait]
    impl WalletReader for Scripted {
        async fn lookup(&self, _: &str) -> Result<Option<(u64, u64, PaymentState)>, String> {
            let mut answers = self.0.lock().unwrap();
            if answers.len() > 1 {
                Ok(answers.remove(0))
            } else {
                Ok(answers[0])
            }
        }
    }

    fn observer(answers: Vec<Option<(u64, u64, PaymentState)>>) -> WalletObserver {
        WalletObserver::new(vec![(
            "alice".to_owned(),
            Arc::new(Scripted(Mutex::new(answers))) as Arc<dyn WalletReader>,
        )])
        .with_poll(Duration::from_millis(1))
    }

    #[tokio::test]
    async fn wait_terminal_polls_until_settled_and_records_every_observation() {
        let observer = observer(vec![
            None,
            Some((1000, 1, PaymentState::Pending)),
            Some((1000, 1, PaymentState::Settled)),
        ]);

        let settled = observer
            .wait_terminal("alice", "o1", "hash", Duration::from_secs(5))
            .await
            .unwrap();

        assert_eq!(settled.state, PaymentState::Settled);
        assert_eq!(settled.amount_sats, 1000);
        assert_eq!(settled.fee_sats, 1);
        assert_eq!(observer.snapshot().len(), 3);
        assert_eq!(observer.snapshot()[0].state, PaymentState::Unknown);
    }

    #[tokio::test]
    async fn wait_terminal_reports_the_last_state_on_timeout_and_unknown_wallets() {
        let observer = observer(vec![Some((5, 0, PaymentState::Pending))]);
        let err = observer
            .wait_terminal("alice", "o1", "hash", Duration::from_millis(5))
            .await
            .unwrap_err();
        assert!(matches!(
            err,
            WalletObserverError::NotSettled {
                last: PaymentState::Pending,
                ..
            }
        ));

        let err = observer.observe("carol", "o1", "hash").await.unwrap_err();
        assert!(matches!(err, WalletObserverError::UnknownWallet { .. }));
    }

    /// A hold-invoice payment is pending from funding until release:
    /// `wait_known` must return that pending observation instead of waiting
    /// for a settlement that cannot happen yet.
    #[tokio::test]
    async fn wait_known_returns_a_pending_hold_payment() {
        let observer = observer(vec![None, Some((1000, 2, PaymentState::Pending))]);

        let known = observer
            .wait_known("alice", "o1", "hash", Duration::from_secs(5))
            .await
            .unwrap();

        assert_eq!(known.state, PaymentState::Pending);
        assert_eq!(known.amount_sats, 1000);
        assert_eq!(known.fee_sats, 2);
    }

    #[tokio::test]
    async fn wait_known_times_out_when_the_wallet_never_learns_the_payment() {
        let observer = observer(vec![None]);

        let err = observer
            .wait_known("alice", "o1", "hash", Duration::from_millis(5))
            .await
            .unwrap_err();

        assert!(matches!(
            err,
            WalletObserverError::NotSettled {
                last: PaymentState::Unknown,
                ..
            }
        ));
    }

    #[test]
    fn nwc_reader_rejects_bad_uris_without_network() {
        assert!(NwcReader::new("alice", &Secret::new("nope"), Duration::from_secs(1)).is_err());
    }

    #[test]
    fn states_map_from_nwc() {
        assert_eq!(
            map_state(Some(TransactionState::Settled)),
            PaymentState::Settled
        );
        assert_eq!(
            map_state(Some(TransactionState::Accepted)),
            PaymentState::Pending
        );
        assert_eq!(
            map_state(Some(TransactionState::Expired)),
            PaymentState::Failed
        );
        assert_eq!(map_state(None), PaymentState::Unknown);
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod amount_tests {
    use super::*;

    /// Wallets answer in millisats. Truncating the remainder would report a
    /// payment as smaller than it was, and the spend accounting believes it.
    #[test]
    fn a_sub_satoshi_remainder_counts_as_a_whole_satoshi() {
        assert_eq!(sats_from_msats(0), 0);
        assert_eq!(sats_from_msats(1_000), 1);
        assert_eq!(sats_from_msats(1_001), 2);
        assert_eq!(sats_from_msats(999), 1);
    }
}
