//! Mortsom environment: Docker relay lifecycle, external Mostro daemon
//! readiness checks and wallet provisioning over NWC or a local LND node.

pub mod config;
pub mod daemon;
pub mod daemon_settings;
pub mod docker;
pub mod lnd;
pub mod lnd_target;
pub mod ports;
pub mod relay;
pub mod relay_url;
pub mod wallet;

pub use config::{MostroConfig, MostroMode, RelayConfig, WalletConfig, WalletsConfig};
pub use daemon::{DaemonInfo, DaemonProbe, RelayProbe};
pub use daemon_settings::{DaemonSettings, TimeoutLimits, evaluate_settings};
pub use docker::{ContainerRuntime, ContainerSpec, DockerCli, DockerError};
pub use lnd::{LndError, LndWallet};
pub use relay::{
    RelayError, RelayHandle, RelayTeardown, restart_relay, start_relay, teardown_relay,
};
pub use wallet::{WalletProbe, WalletProbeError};
