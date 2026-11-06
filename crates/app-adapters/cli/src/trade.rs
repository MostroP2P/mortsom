//! Trade steps: fiat sent, release, rating and disputes.

use mortsom_app_contract::{AdapterError, DeviceContext, OrderReference};
use mortsom_cli_driver::Call;

use crate::access::run;

/// `order fiat-sent`.
pub(crate) async fn fiat_sent(
    device: &DeviceContext<'_>,
    order: &OrderReference,
) -> Result<(), AdapterError> {
    step(device, "fiat-sent", order).await
}

/// `order release`.
pub(crate) async fn release(
    device: &DeviceContext<'_>,
    order: &OrderReference,
) -> Result<(), AdapterError> {
    step(device, "release", order).await
}

async fn step(
    device: &DeviceContext<'_>,
    verb: &str,
    order: &OrderReference,
) -> Result<(), AdapterError> {
    let call = Call::node(&["order", verb])
        .arg("order", order.id.clone())
        .confirmed();
    run(device, call).await.map(|_| ())
}

/// `order rate`: rates the counterpart of a completed order.
pub(crate) async fn rate(
    device: &DeviceContext<'_>,
    order: &OrderReference,
    score: u8,
) -> Result<(), AdapterError> {
    if !(1..=5).contains(&score) {
        return Err(AdapterError::contract(format!(
            "a rating is 1 to 5 stars, not {score}"
        )));
    }
    let call = Call::node(&["order", "rate"])
        .arg("order", order.id.clone())
        .arg("rating", score.to_string())
        .confirmed();
    run(device, call).await.map(|_| ())
}

/// `dispute open`: opens a dispute on a funded order.
pub(crate) async fn open_dispute(
    device: &DeviceContext<'_>,
    order: &OrderReference,
) -> Result<(), AdapterError> {
    let call = Call::node(&["dispute", "open"])
        .arg("order", order.id.clone())
        .confirmed();
    run(device, call).await.map(|_| ())
}
