//! PR smoke suite (spec §9.1).

pub mod happy_buy;
pub mod happy_sell;

/// Keeps registrations linked.
pub fn link() {
    happy_buy::link();
    happy_sell::link();
}
