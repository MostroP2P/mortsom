//! Mortsom scenario catalogue (spec §9): smoke, regression, recovery and
//! adversarial scenarios expressed against the scenario kit. Scenarios are
//! typed domain narratives; they self-register with the kit's registry.

pub mod smoke;

/// Forces the linker to keep every scenario registration.
pub fn link() {
    smoke::link();
}
