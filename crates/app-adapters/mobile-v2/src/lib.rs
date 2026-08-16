//! Mortsom adapter for the second-generation MostroP2P mobile application
//! (`../mobilev2`, package `foundation.mostro.app`). It knows that app's
//! build, bootstrap, identifiers, navigation and state labels; the scenario
//! kit only sees the shared `AppAdapter` contract (spec §12.1, Phase 7).
//!
//! The identifiers mirror `docs/mobile-v2-inventory.md`, the automation
//! contract the v2 app must expose (the same `AutomationId` mechanism as
//! the classic app). The adapter fails with an automation-contract error,
//! never a false pass, when the build under test lacks them.

pub mod adapter;
pub mod build;
pub mod screens;
pub mod selectors;
pub mod states;

pub use adapter::{MobileV2Adapter, PROFILE};
