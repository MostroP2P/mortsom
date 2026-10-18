//! Mortsom scenario catalogue (spec §9): smoke, regression, recovery and
//! adversarial scenarios expressed against the scenario kit. Scenarios are
//! typed domain narratives; they self-register with the kit's registry.

pub mod bond;
pub mod cancellation;
pub mod dispute;
pub mod expiration;
pub mod flow;
pub mod happy;
pub mod payment_failure;
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
        $crate::register_scenario! {
            name: $name,
            body: $body,
            tags: $tags,
            requires: $requires,
            timeout_secs: $timeout,
            moves_value: $moves_value,
            needs_app_wallet: false,
            needs_solver: false,
            needs: &[],
        }
    };
    (
        name: $name:literal,
        body: $body:path,
        tags: $tags:expr,
        requires: $requires:expr,
        timeout_secs: $timeout:expr,
        moves_value: $moves_value:expr,
        needs_app_wallet: $needs_app_wallet:expr $(,)?
    ) => {
        $crate::register_scenario! {
            name: $name,
            body: $body,
            tags: $tags,
            requires: $requires,
            timeout_secs: $timeout,
            moves_value: $moves_value,
            needs_app_wallet: $needs_app_wallet,
            needs_solver: false,
            needs: &[],
        }
    };
    (
        name: $name:literal,
        body: $body:path,
        tags: $tags:expr,
        requires: $requires:expr,
        timeout_secs: $timeout:expr,
        moves_value: $moves_value:expr,
        needs_app_wallet: $needs_app_wallet:expr,
        needs_solver: $needs_solver:expr $(,)?
    ) => {
        $crate::register_scenario! {
            name: $name,
            body: $body,
            tags: $tags,
            requires: $requires,
            timeout_secs: $timeout,
            moves_value: $moves_value,
            needs_app_wallet: $needs_app_wallet,
            needs_solver: $needs_solver,
            needs: &[],
        }
    };
    (
        name: $name:literal,
        body: $body:path,
        tags: $tags:expr,
        requires: $requires:expr,
        timeout_secs: $timeout:expr,
        moves_value: $moves_value:expr,
        needs: $needs:expr $(,)?
    ) => {
        $crate::register_scenario! {
            name: $name,
            body: $body,
            tags: $tags,
            requires: $requires,
            timeout_secs: $timeout,
            moves_value: $moves_value,
            needs_app_wallet: false,
            needs_solver: false,
            needs: $needs,
        }
    };
    (
        name: $name:literal,
        body: $body:path,
        tags: $tags:expr,
        requires: $requires:expr,
        timeout_secs: $timeout:expr,
        moves_value: $moves_value:expr,
        needs_app_wallet: $needs_app_wallet:expr,
        needs_solver: $needs_solver:expr,
        needs: $needs:expr $(,)?
    ) => {
        inventory::submit! {
            ::mortsom_scenario_kit::registry::ScenarioDef {
                name: $name,
                tags: $tags,
                requires: $requires,
                timeout_secs: $timeout,
                moves_value: $moves_value,
                needs_app_wallet: $needs_app_wallet,
                needs_solver: $needs_solver,
                needs: $needs,
                run: |ctx| Box::pin($body(ctx)),
            }
        }
    };
}

/// Forces the linker to keep every scenario registration.
pub fn link() {
    smoke::link();
    happy::link();
    cancellation::link();
    dispute::link();
    expiration::link();
    payment_failure::link();
    bond::link();
    recovery::link();
    regression::link();
}

#[cfg(test)]
#[path = "catalogue_tests.rs"]
mod catalogue_tests;
#[cfg(test)]
mod flow_fixture;
