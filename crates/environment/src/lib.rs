//! Mortsom environment: Docker relay lifecycle, external Mostro daemon
//! readiness checks and NWC wallet provisioning.

pub mod config;

pub use config::{MostroConfig, MostroMode, RelayConfig, WalletMode, WalletsConfig};
