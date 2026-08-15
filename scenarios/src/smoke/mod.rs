//! PR smoke suite (spec §9.1).

pub mod happy_sell;

/// Keeps registrations linked.
pub fn link() {
    happy_sell::link();
}
