//! Happy-path family (spec §8, tag `happy-path`) beyond the two smoke
//! trades: the other shapes a trade that works can take. `happy_sell` and
//! `happy_buy` live in [`crate::smoke`] and carry the same family tag.

pub mod full_trade_with_rating;
pub mod market_price_order;
pub mod range;
pub mod range_buy_order;
pub mod range_order;

/// Deadline for a trade that also settles a remainder or a rating.
pub const EXTENDED_TRADE_TIMEOUT_SECS: u64 = 35 * 60;

/// Keeps registrations linked.
pub fn link() {
    range_order::link();
    range_buy_order::link();
    market_price_order::link();
    full_trade_with_rating::link();
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
