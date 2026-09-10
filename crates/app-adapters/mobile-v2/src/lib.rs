//! Mortsom adapter for the second-generation MostroP2P application
//! (`../mobilev2`), driven on Linux and Web. It knows that app's bootstrap,
//! identifiers, navigation and state labels; the scenario kit only sees the
//! shared `AppAdapter` contract (spec §12.1, Phase 7).
//!
//! The identifiers mirror `docs/history/mobile-v2-inventory.md`, the
//! automation contract the app must expose. The adapter fails with an
//! automation-contract error, never a false pass, when the build under test
//! lacks them.

pub mod adapter;
pub mod screens;
pub mod selectors;
pub mod states;

pub use adapter::{MobileV2Adapter, PROFILE};
