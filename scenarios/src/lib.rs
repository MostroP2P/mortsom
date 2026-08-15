//! Mortsom scenario catalogue (spec §9): smoke, regression, recovery and
//! adversarial scenarios expressed against the scenario kit. Scenarios are
//! typed domain narratives; they self-register with the kit's registry.

pub mod flow;
pub mod recovery;
pub mod regression;
pub mod smoke;

/// Registers a scenario body with the kit's inventory.
#[macro_export]
macro_rules! register_scenario {
    (
        name: $name:literal,
        body: $body:path,
        tags: $tags:expr,
        requires: $requires:expr,
        timeout_secs: $timeout:expr,
        moves_value: $moves_value:expr $(,)?
    ) => {
        inventory::submit! {
            ::mortsom_scenario_kit::registry::ScenarioDef {
                name: $name,
                tags: $tags,
                requires: $requires,
                timeout_secs: $timeout,
                moves_value: $moves_value,
                run: |ctx| Box::pin($body(ctx)),
            }
        }
    };
}

/// Forces the linker to keep every scenario registration.
pub fn link() {
    smoke::link();
    recovery::link();
    regression::link();
}
