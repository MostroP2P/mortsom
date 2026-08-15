//! Regression scenarios (spec §9.2, Phase 5.4): cancellation paths,
//! expiration and unauthorized-action UX.

pub mod cancel;
pub mod permissions;

/// Keeps registrations linked.
pub fn link() {
    cancel::link();
    permissions::link();
}
