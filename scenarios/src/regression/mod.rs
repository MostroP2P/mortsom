//! Regression scenarios (spec §9.2, Phase 5.4): unauthorized-action UX.
//! Cancellations and expirations are their own families in
//! [`crate::cancellation`] and [`crate::expiration`].

pub mod permissions;

/// Keeps registrations linked.
pub fn link() {
    permissions::link();
}
