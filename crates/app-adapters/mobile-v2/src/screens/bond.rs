//! The anti-abuse bond in the v2 app: the pay-bond screen a take or a
//! creation is parked on, the walk away from it, the winner's claim of a
//! slashed bond's share, and the loser's durable notice.

use mortsom_app_contract::{
    AdapterError, BondClaim, DeviceContext, OrderReference, PaymentHandoff, PaymentPath,
};
use mortsom_common::Secret;

use super::nav::go_home;
use super::orders::{open_trade, verify_order_id};
use super::{expect, first_present, label, present, tap, type_into};
use crate::selectors::Ids;

/// Opens the pay-bond screen of `order`, unless it is already on screen:
/// from the trade (a taker's) or the maker's own order, through the
/// deposit action each offers while the bond is outstanding. The screen
/// is known by its app bar, which both of its faces carry.
async fn open_bond_screen(
    device: &DeviceContext<'_>,
    order: &OrderReference,
) -> Result<(), AdapterError> {
    if !present(device, Ids::BOND_ORDER_ID).await? {
        open_trade(device, order).await?;
        let deposit = first_present(device, &[Ids::TRADE_PAY_BOND, Ids::MY_ORDER_PAY_BOND])
            .await?
            .ok_or_else(|| {
                AdapterError::contract(format!(
                    "order {} offers no deposit action; is its bond still outstanding?",
                    order.id
                ))
            })?;
        tap(device, deposit).await?;
        expect(device, Ids::BOND_ORDER_ID).await?;
    }
    verify_order_id(device, Ids::BOND_ORDER_ID, order).await
}

/// The bond bolt11 as the screen shows it: its own readout without a
/// wallet, or the shared pay widget's with one connected. With the
/// explainer open the screen draws neither, so it is folded first.
async fn bond_invoice_text(device: &DeviceContext<'_>) -> Result<String, AdapterError> {
    let readouts = [Ids::BOND_INVOICE_TEXT, Ids::PAY_INVOICE_TEXT];
    let mut readout = first_present(device, &readouts).await?;
    if readout.is_none() && present(device, Ids::BOND_EXPLAINER).await? {
        tap(device, Ids::BOND_EXPLAINER).await?;
        readout = first_present(device, &readouts).await?;
    }
    let readout = readout
        .ok_or_else(|| AdapterError::contract("the pay-bond screen shows no invoice readout"))?;
    Ok(label(device, readout).await?.trim().to_owned())
}

/// Hands off the anti-abuse bond invoice, the way the escrow is handed
/// off: read for the caller's node to pay, or paid by the connected
/// wallet. The bond locks when the daemon sees the payment, and the app
/// then moves on by itself: a taker to the trade's first step, a maker
/// to the published order.
pub async fn pay_bond_invoice(
    device: &DeviceContext<'_>,
    order: &OrderReference,
    path: PaymentPath,
) -> Result<PaymentHandoff, AdapterError> {
    open_bond_screen(device, order).await?;
    let invoice = bond_invoice_text(device).await?;
    if invoice.is_empty() {
        return Err(AdapterError::product(
            "the pay-bond screen shows no invoice, so there is no bond to pay",
        ));
    }
    if path == PaymentPath::ExternalWallet {
        return Ok(PaymentHandoff {
            kind: PaymentPath::ExternalWallet,
            invoice: Some(Secret::new(invoice)),
        });
    }
    tap(device, Ids::PAY_NWC).await?;
    let next = first_present(
        device,
        &[Ids::INVOICE_TEXT, Ids::PAY_INVOICE_TEXT, Ids::ORDER_STATUS],
    )
    .await?;
    if next.is_none() {
        return Err(AdapterError::contract(
            "the bond payment did not hand over to the next step",
        ));
    }
    Ok(PaymentHandoff {
        kind: PaymentPath::Nwc,
        invoice: Some(Secret::new(invoice)),
    })
}

/// Walks away on the pay-bond screen: a taker's cancel of the take, or a
/// maker's cancel of an order never published. The app asks first, sends
/// the daemon the cancel once confirmed, and returns to the book. A node
/// that refuses the maker's cancel makes the app offer to drop the order
/// from this device only, which is no cancel at all: that is the product's
/// answer, and nothing is dropped.
pub async fn decline_bond(
    device: &DeviceContext<'_>,
    order: &OrderReference,
) -> Result<(), AdapterError> {
    open_bond_screen(device, order).await?;
    tap(device, Ids::BOND_CANCEL).await?;
    tap(device, Ids::BOND_CANCEL_CONFIRM).await?;
    match first_present(device, &[Ids::ORDER_ADD_FAB, Ids::BOND_REMOVE_FROM_DEVICE]).await? {
        Some(id) if id == Ids::ORDER_ADD_FAB => Ok(()),
        Some(_) => Err(AdapterError::product(format!(
            "the node refused to cancel order {} on its pay-bond screen",
            order.id
        ))),
        None => Err(AdapterError::contract(
            "leaving the pay-bond screen returned neither to the book nor to the node's refusal",
        )),
    }
}

/// Opens the claim screen of `order` from the trades list: through the
/// trade detail's banner while the trade still exists, or straight away
/// when the claim is all that is left of it. `false` when the row shows
/// no claim.
async fn open_claim_screen(
    device: &DeviceContext<'_>,
    order: &OrderReference,
) -> Result<bool, AdapterError> {
    if present(device, Ids::BOND_CLAIM_STATUS).await? {
        verify_order_id(device, Ids::BOND_CLAIM_ORDER_ID, order).await?;
        return Ok(true);
    }
    go_home(device).await?;
    tap(device, Ids::NAV_TRADES).await?;
    tap(device, &Ids::trades_item(&order.id)).await?;
    let landed = first_present(
        device,
        &[
            Ids::BOND_CLAIM_STATUS,
            Ids::TRADE_BOND_CLAIM,
            Ids::ORDER_STATUS,
        ],
    )
    .await?;
    match landed {
        Some(id) if id == Ids::BOND_CLAIM_STATUS => {}
        // The detail draws its status before the claim banner, which
        // arrives with the claim itself: the banner is given a moment.
        Some(id)
            if id == Ids::TRADE_BOND_CLAIM
                || super::listed_within(device, Ids::TRADE_BOND_CLAIM, device.ui_timeout)
                    .await? =>
        {
            verify_order_id(device, Ids::ORDER_ID, order).await?;
            tap(device, Ids::TRADE_BOND_CLAIM_OPEN).await?;
            expect(device, Ids::BOND_CLAIM_STATUS).await?;
        }
        Some(_) => return Ok(false),
        None => {
            return Err(AdapterError::contract(
                "the trade opened neither its detail nor a claim",
            ));
        }
    }
    verify_order_id(device, Ids::BOND_CLAIM_ORDER_ID, order).await?;
    Ok(true)
}

/// Reads the claim the app offers for `order`: the share on offer and the
/// phase the screen renders.
pub async fn claim(
    device: &DeviceContext<'_>,
    order: &OrderReference,
) -> Result<Option<BondClaim>, AdapterError> {
    if !open_claim_screen(device, order).await? {
        return Ok(None);
    }
    let amount = label(device, Ids::BOND_CLAIM_AMOUNT).await?;
    let digits: String = amount.chars().filter(char::is_ascii_digit).collect();
    let amount_sats = digits.parse().map_err(|_| {
        AdapterError::contract(format!(
            "`bond.claim.amount` reads `{}`, not a sat amount",
            amount.trim()
        ))
    })?;
    let status = label(device, Ids::BOND_CLAIM_STATUS)
        .await?
        .trim()
        .to_ascii_lowercase();
    Ok(Some(BondClaim {
        amount_sats,
        status,
    }))
}

/// Types `invoice` into the claim form and sends it. With a wallet
/// connected the form hides behind the wallet widget, and the manual way
/// in is taken first. Returns the phase the screen reads afterwards.
pub async fn submit_claim_invoice(
    device: &DeviceContext<'_>,
    order: &OrderReference,
    invoice: &Secret,
) -> Result<String, AdapterError> {
    if !open_claim_screen(device, order).await? {
        return Err(AdapterError::product(format!(
            "the app offers no claim for order {}",
            order.id
        )));
    }
    if !present(device, Ids::BOND_CLAIM_TEXT).await?
        && present(device, Ids::BOND_CLAIM_MANUAL).await?
    {
        tap(device, Ids::BOND_CLAIM_MANUAL).await?;
    }
    type_into(device, Ids::BOND_CLAIM_TEXT, invoice.expose()).await?;
    tap(device, Ids::BOND_CLAIM_SUBMIT).await?;
    Ok(label(device, Ids::BOND_CLAIM_STATUS)
        .await?
        .trim()
        .to_ascii_lowercase())
}

/// The cause the trade detail shows for this user's own slashed bond, or
/// `None` when the trade carries no notice or is no longer listed (a
/// timeout slash wipes the taker's row).
pub async fn slash_notice(
    device: &DeviceContext<'_>,
    order: &OrderReference,
) -> Result<Option<String>, AdapterError> {
    go_home(device).await?;
    tap(device, Ids::NAV_TRADES).await?;
    if !super::listed_within(device, &Ids::trades_item(&order.id), device.ui_timeout).await? {
        return Ok(None);
    }
    open_trade(device, order).await?;
    if !present(device, Ids::TRADE_BOND_SLASHED).await? {
        return Ok(None);
    }
    Ok(Some(
        label(device, Ids::TRADE_BOND_SLASHED)
            .await?
            .trim()
            .to_ascii_lowercase(),
    ))
}
