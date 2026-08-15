//! Mortsom environment: Docker relay lifecycle, external Mostro daemon
//! readiness checks and NWC wallet provisioning.

pub mod config;
pub mod daemon;
pub mod wallet;

pub use config::{MostroConfig, MostroMode, RelayConfig, WalletMode, WalletsConfig};
pub use daemon::{DaemonInfo, DaemonProbe, RelayProbe};
pub use wallet::{WalletProbe, WalletProbeError};
