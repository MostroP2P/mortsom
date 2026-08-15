//! Mortsom environment: Docker relay lifecycle, external Mostro daemon
//! readiness checks and NWC wallet provisioning.

pub mod config;
pub mod daemon;
pub mod daemon_settings;
pub mod docker;
pub mod ports;
pub mod relay;
pub mod relay_url;
pub mod wallet;

pub use config::{MostroConfig, MostroMode, RelayConfig, WalletMode, WalletsConfig};
pub use daemon::{DaemonInfo, DaemonProbe, RelayProbe};
pub use daemon_settings::{DaemonSettings, TimeoutLimits, evaluate_settings};
pub use docker::{ContainerRuntime, ContainerSpec, DockerCli, DockerError};
pub use relay::{
    RelayError, RelayHandle, RelayTeardown, restart_relay, start_relay, teardown_relay,
};
pub use wallet::{WalletProbe, WalletProbeError};
