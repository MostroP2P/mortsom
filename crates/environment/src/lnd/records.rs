//! Count actual hash-scoped ledger records, never streamed observations or HTLCs.
use super::{LndError, RPC_TIMEOUT, amounts, requests};
use fedimint_tonic_lnd::lnrpc;
use std::future::Future;

pub(super) const PAGE_SIZE: u64 = 1000;
const MAX_PAGES: usize = 100;

pub(super) struct Record {
    index: u64,
    hash: Vec<u8>,
    settled: bool,
    principal_msats: i64,
    amp: bool,
}
pub(super) struct Page {
    records: Vec<Record>,
    last_index: u64,
}

pub(super) fn outgoing(
    label: &str,
    response: lnrpc::ListPaymentsResponse,
) -> Result<Page, LndError> {
    let records = response
        .payments
        .into_iter()
        .map(|payment| {
            Ok(Record {
                index: payment.payment_index,
                hash: requests::hash_bytes(label, &payment.payment_hash)?,
                settled: payment.status == lnrpc::payment::PaymentStatus::Succeeded as i32,
                principal_msats: payment.value_msat,
                amp: false,
            })
        })
        .collect::<Result<_, LndError>>()?;
    Ok(Page {
        records,
        last_index: response.last_index_offset,
    })
}

pub(super) fn incoming(response: lnrpc::ListInvoiceResponse) -> Page {
    Page {
        records: response
            .invoices
            .into_iter()
            .map(|invoice| Record {
                index: invoice.add_index,
                hash: invoice.r_hash,
                settled: invoice.state == lnrpc::invoice::InvoiceState::Settled as i32,
                principal_msats: invoice.amt_paid_msat,
                amp: invoice.is_amp,
            })
            .collect(),
        last_index: response.last_index_offset,
    }
}

pub(super) async fn count<F, Fut>(label: &str, hash: &str, mut fetch: F) -> Result<usize, LndError>
where
    F: FnMut(u64) -> Fut,
    Fut: Future<Output = Result<Page, LndError>>,
{
    let wanted = requests::hash_bytes(label, hash)?;
    tokio::time::timeout(RPC_TIMEOUT, async {
        let mut offset = 0;
        let mut count = 0;
        for _ in 0..MAX_PAGES {
            let page = fetch(offset).await?;
            if page.records.is_empty() {
                return Ok(count);
            }
            if page.records.len() > PAGE_SIZE as usize {
                return Err(invalid(label, "ledger page exceeded the requested limit"));
            }
            let mut last = offset;
            for record in page.records {
                if record.index <= last {
                    return Err(invalid(label, "duplicate or unordered record index"));
                }
                last = record.index;
                if record.hash == wanted && record.settled {
                    if record.amp {
                        return Err(invalid(
                            label,
                            "AMP invoices require separate settlement accounting",
                        ));
                    }
                    amounts::whole_sats(
                        label,
                        "ledger principal millisatoshi",
                        record.principal_msats,
                    )?;
                    count += 1;
                }
            }
            if page.last_index != last {
                return Err(invalid(
                    label,
                    "pagination cursor does not identify the final record",
                ));
            }
            offset = last;
        }
        Err(invalid(
            label,
            "ledger pagination exceeded its record limit",
        ))
    })
    .await
    .map_err(|_| LndError::Timeout {
        label: label.into(),
        operation: "list settlement records",
        timeout: RPC_TIMEOUT,
    })?
}

fn invalid(label: &str, message: &'static str) -> LndError {
    LndError::Failed {
        label: label.into(),
        operation: "list settlement records",
        message: message.into(),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    const HASH: &str = "1111111111111111111111111111111111111111111111111111111111111111";
    fn record(index: u64, byte: u8, settled: bool) -> Record {
        Record {
            index,
            hash: vec![byte; 32],
            settled,
            principal_msats: 1000,
            amp: false,
        }
    }
    #[tokio::test]
    async fn multiple_htlc_attempts_remain_one_payment_ledger_record() {
        let page = outgoing(
            "a",
            lnrpc::ListPaymentsResponse {
                payments: vec![lnrpc::Payment {
                    payment_index: 1,
                    payment_hash: HASH.into(),
                    value_msat: 1000,
                    status: lnrpc::payment::PaymentStatus::Succeeded as i32,
                    htlcs: vec![lnrpc::HtlcAttempt::default(), lnrpc::HtlcAttempt::default()],
                    ..Default::default()
                }],
                last_index_offset: 1,
                ..Default::default()
            },
        )
        .unwrap();
        let mut page = Some(page);
        assert_eq!(
            count("a", HASH, |_| {
                let page = page.take().unwrap_or(Page {
                    records: vec![],
                    last_index: 1,
                });
                async move { Ok(page) }
            })
            .await
            .unwrap(),
            1
        );
    }

    #[tokio::test]
    async fn incoming_invoice_records_exclude_other_hashes_and_refuse_amp_or_fractional_settlement()
    {
        for (amp, paid, valid) in [
            (false, 1000, true),
            (true, 1000, false),
            (false, 1001, false),
        ] {
            let mut page = Some(incoming(lnrpc::ListInvoiceResponse {
                invoices: vec![lnrpc::Invoice {
                    add_index: 1,
                    r_hash: vec![0x11; 32],
                    is_amp: amp,
                    state: lnrpc::invoice::InvoiceState::Settled as i32,
                    amt_paid_msat: paid,
                    ..Default::default()
                }],
                last_index_offset: 1,
                ..Default::default()
            }));
            let result = count("a", HASH, |_| {
                let page = page.take().unwrap_or(Page {
                    records: vec![],
                    last_index: 1,
                });
                async move { Ok(page) }
            })
            .await;
            assert_eq!(result.is_ok(), valid);
            if valid {
                assert_eq!(result.unwrap(), 1);
            }
        }
    }

    #[tokio::test]
    async fn counts_matching_settlements_across_pages_without_counting_other_hashes_or_attempts() {
        let count = count("a", HASH, |offset| async move {
            Ok(match offset {
                0 => Page {
                    records: vec![record(1, 0x22, true), record(2, 0x11, false)],
                    last_index: 2,
                },
                2 => Page {
                    records: vec![record(3, 0x11, true)],
                    last_index: 3,
                },
                _ => Page {
                    records: vec![],
                    last_index: offset,
                },
            })
        })
        .await
        .unwrap();
        assert_eq!(count, 1);
    }
    #[tokio::test]
    async fn a_second_settlement_is_counted_but_a_repeated_index_is_an_error() {
        assert_eq!(
            count("a", HASH, |offset| async move {
                Ok(if offset == 0 {
                    Page {
                        records: vec![record(1, 0x11, true), record(2, 0x11, true)],
                        last_index: 2,
                    }
                } else {
                    Page {
                        records: vec![],
                        last_index: offset,
                    }
                })
            })
            .await
            .unwrap(),
            2
        );
        assert!(
            count("a", HASH, |_| async {
                Ok(Page {
                    records: vec![record(1, 0x11, true)],
                    last_index: 1,
                })
            })
            .await
            .is_err()
        );
    }
    #[tokio::test]
    async fn unbounded_pagination_and_malformed_cursors_fail_instead_of_claiming_uniqueness() {
        assert!(
            count("a", HASH, |offset| async move {
                Ok(Page {
                    records: vec![record(offset + 1, 0x22, false)],
                    last_index: offset + 1,
                })
            })
            .await
            .is_err()
        );
        assert!(
            count("a", HASH, |_| async {
                Ok(Page {
                    records: vec![record(1, 0x11, true)],
                    last_index: 2,
                })
            })
            .await
            .is_err()
        );
    }
}
