//! Regression scenarios (spec §9.2, Phase 5.4): expiration and
//! unauthorized-action UX. Cancellations are their own family in
//! [`crate::cancellation`].

pub mod expiration;
pub mod permissions;

/// Keeps registrations linked.
pub fn link() {
    expiration::link();
    permissions::link();
}
