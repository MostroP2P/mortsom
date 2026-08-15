//! Recovery scenarios (spec §9.2, Phase 5): the same trade journeys
//! interrupted by app restarts, backgrounding, connectivity loss, relay
//! restarts, replayed daemon events and wallet-side hiccups. Each asserts
//! the restored state and, for restarts, the absence of duplicated
//! protocol actions.

pub mod interruption;
pub mod restart;
pub mod wallet;

/// Deadline for a scenario that never moves value.
pub const NO_VALUE_TIMEOUT_SECS: u64 = 20 * 60;
/// Deadline for a scenario that funds and completes a trade.
pub const TRADE_TIMEOUT_SECS: u64 = 35 * 60;

/// Keeps registrations linked.
pub fn link() {
    restart::link();
    interruption::link();
    wallet::link();
}
