//! Bounded, injectable LND RPC requests for public hash-scoped operations.
use super::LndError;
use fedimint_tonic_lnd::{
    invoicesrpc, lnrpc, routerrpc,
    tonic::{Code, Status},
};
use std::{future::Future, time::Duration};

pub(super) fn hash_bytes(label: &str, hash: &str) -> Result<Vec<u8>, LndError> {
    super::hex_to_bytes(hash)
        .filter(|bytes| bytes.len() == 32)
        .ok_or_else(|| LndError::InvalidHash {
            label: label.into(),
        })
}

pub(super) async fn track<F, Fut>(
    label: &str,
    hash: &str,
    timeout: Duration,
    send: F,
) -> Result<Option<lnrpc::Payment>, LndError>
where
    F: FnOnce(routerrpc::TrackPaymentRequest) -> Fut,
    Fut: Future<Output = Result<Option<lnrpc::Payment>, Status>>,
{
    let request = routerrpc::TrackPaymentRequest {
        payment_hash: hash_bytes(label, hash)?,
        no_inflight_updates: false,
    };
    match bounded(label, "track_payment_v2", timeout, send(request)).await {
        Err(LndError::Rpc {
            code: Code::NotFound,
            ..
        }) => Ok(None),
        result => result,
    }
}

pub(super) async fn cancel<F, Fut>(
    label: &str,
    hash: &str,
    timeout: Duration,
    send: F,
) -> Result<(), LndError>
where
    F: FnOnce(invoicesrpc::CancelInvoiceMsg) -> Fut,
    Fut: Future<Output = Result<(), Status>>,
{
    let request = invoicesrpc::CancelInvoiceMsg {
        payment_hash: hash_bytes(label, hash)?,
    };
    bounded(label, "cancel_invoice", timeout, send(request)).await
}

async fn bounded<T>(
    label: &str,
    operation: &'static str,
    timeout: Duration,
    future: impl Future<Output = Result<T, Status>>,
) -> Result<T, LndError> {
    tokio::time::timeout(timeout, future)
        .await
        .map_err(|_| LndError::Timeout {
            label: label.into(),
            operation,
            timeout,
        })?
        .map_err(|cause| LndError::Rpc {
            label: label.into(),
            operation,
            code: cause.code(),
        })
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    const HASH: &str = "1111111111111111111111111111111111111111111111111111111111111111";

    #[test]
    fn invalid_unicode_or_wrong_length_hashes_cannot_reach_an_rpc() {
        for hash in [
            "0é0",
            "abcd",
            "",
            "xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx",
        ] {
            assert!(matches!(
                hash_bytes("a", hash),
                Err(LndError::InvalidHash { .. })
            ));
        }
    }

    #[tokio::test]
    async fn an_inflight_hold_is_observable_before_release() {
        let observed = track(
            "alice",
            HASH,
            Duration::from_millis(30),
            |request| async move {
                assert_eq!(request.payment_hash, vec![0x11; 32]);
                if request.no_inflight_updates {
                    std::future::pending::<()>().await;
                }
                Ok(Some(lnrpc::Payment {
                    status: lnrpc::payment::PaymentStatus::InFlight as i32,
                    value_msat: 1000,
                    ..Default::default()
                }))
            },
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(
            observed.status,
            lnrpc::payment::PaymentStatus::InFlight as i32
        );
    }

    #[tokio::test]
    async fn track_bounds_connection_and_first_update_and_keeps_not_found_typed() {
        assert!(matches!(
            track("alice", HASH, Duration::from_millis(10), |_| {
                std::future::pending()
            })
            .await,
            Err(LndError::Timeout {
                operation: "track_payment_v2",
                ..
            })
        ));
        assert!(
            track("alice", HASH, Duration::from_secs(1), |_| async {
                Err(Status::not_found("hidden"))
            })
            .await
            .unwrap()
            .is_none()
        );
        assert!(matches!(
            track("alice", HASH, Duration::from_secs(1), |_| async {
                Err(Status::permission_denied("private detail"))
            })
            .await,
            Err(LndError::Rpc {
                code: Code::PermissionDenied,
                ..
            })
        ));
    }

    #[tokio::test]
    async fn cancel_sends_only_the_exact_hash_and_never_retries_a_failure() {
        let mut calls = 0;
        let result = cancel("bob", HASH, Duration::from_secs(1), |request| {
            calls += 1;
            async move {
                assert_eq!(request.payment_hash, vec![0x11; 32]);
                Err(Status::failed_precondition(
                    "already settled, private detail",
                ))
            }
        })
        .await;
        assert_eq!(calls, 1);
        assert!(matches!(
            result,
            Err(LndError::Rpc {
                code: Code::FailedPrecondition,
                ..
            })
        ));
        assert!(matches!(
            cancel("bob", "abcd", Duration::from_secs(1), |_| async {
                panic!("invalid hash must not reach RPC")
            })
            .await,
            Err(LndError::InvalidHash { .. })
        ));
        assert!(matches!(
            cancel("bob", HASH, Duration::from_millis(10), |_| {
                std::future::pending()
            })
            .await,
            Err(LndError::Timeout {
                operation: "cancel_invoice",
                ..
            })
        ));
    }
}
