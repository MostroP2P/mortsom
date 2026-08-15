//! Mortsom adapter for the classic MostroP2P Flutter application (`../mobile`).
//!
//! The adapter is the only layer that knows this application's package,
//! build procedure, bootstrap mechanism, semantic identifiers, navigation
//! and copy (spec §12.1). It drives the app strictly through the UI.

pub mod adapter;
pub mod build;
pub mod screens;
pub mod selectors;

pub use adapter::MobileLegacyAdapter;
pub use build::{APK_RELATIVE_PATH, MORTSOM_ENTRYPOINT, build_command};
pub use selectors::Ids;
