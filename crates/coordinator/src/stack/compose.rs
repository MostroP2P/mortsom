//! Command plans for the stack's Docker Compose project. Nothing here runs a
//! process; the caller hands each plan to a `CommandRunner`.
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::Duration;

use mortsom_android_driver::process::{Command, CommandRunner, Output, run_ok};

use super::StackError;
use super::layout::{Layout, PROJECT};

/// Path of the LND state directory inside the node images.
pub const LND_DIR: &str = "/home/lnd/.lnd";
/// Wallet bitcoind mines into and pays the nodes from.
pub const MINER_WALLET: &str = "mortsom";
/// Services started before the daemon; the daemon needs carol's credentials.
pub const BASE_SERVICES: [&str; 5] = ["bitcoind", "lnd-alice", "lnd-bob", "lnd-carol", "relay"];
/// The daemon's service, behind the `daemon` profile.
pub const DAEMON_SERVICE: &str = "mostro";

const COMMAND_TIMEOUT: Duration = Duration::from_secs(120);

/// Environment every compose invocation carries: the file, the project name
/// and the invoking user's ids so the daemon can read owner-only files.
#[derive(Debug, Clone)]
pub struct Compose {
    file: PathBuf,
    env: BTreeMap<String, String>,
}

impl Compose {
    /// Plans for the project in `layout`; `image` is what the daemon runs.
    pub fn new(layout: &Layout, image: Option<&str>) -> Self {
        let mut env = BTreeMap::from([
            (
                "COMPOSE_FILE".to_owned(),
                layout.compose_file().display().to_string(),
            ),
            ("COMPOSE_PROJECT_NAME".to_owned(), PROJECT.to_owned()),
            (
                "MORTSOM_UID".to_owned(),
                rustix::process::getuid().as_raw().to_string(),
            ),
            (
                "MORTSOM_GID".to_owned(),
                rustix::process::getgid().as_raw().to_string(),
            ),
        ]);
        if let Some(image) = image {
            env.insert("MORTSOM_MOSTRO_IMAGE".to_owned(), image.to_owned());
        }
        Self {
            file: layout.compose_file(),
            env,
        }
    }

    /// The compose file every plan is bound to.
    pub fn file(&self) -> &std::path::Path {
        &self.file
    }

    /// `docker compose <args>` for this project.
    pub fn command(&self, args: &[&str]) -> Command {
        let mut command = Command::new("docker", &["compose"]);
        command
            .args
            .extend(args.iter().map(|arg| (*arg).to_owned()));
        for (key, value) in &self.env {
            command = command.env(key, value);
        }
        command
    }

    /// `bitcoin-cli` inside the chain container.
    pub fn bitcoin_cli(&self, args: &[&str]) -> Command {
        let mut argv = vec![
            "exec",
            "-T",
            "bitcoind",
            "bitcoin-cli",
            "-regtest",
            "-rpcuser=mortsom",
            "-rpcpassword=mortsom",
        ];
        argv.extend_from_slice(args);
        self.command(&argv)
    }

    /// `lncli` inside one node's container.
    pub fn lncli(&self, node: &str, args: &[&str]) -> Command {
        let service = format!("lnd-{node}");
        let lnddir = format!("--lnddir={LND_DIR}");
        let mut argv = vec![
            "exec",
            "-T",
            &service,
            "lncli",
            &lnddir,
            "--network=regtest",
        ];
        argv.extend_from_slice(args);
        self.command(&argv)
    }
}

/// Runs a plan and returns its output whether or not it succeeded, for
/// probes that are expected to fail until a service is up.
pub async fn attempt(runner: &dyn CommandRunner, command: &Command) -> Result<Output, StackError> {
    Ok(runner.run(command, COMMAND_TIMEOUT).await?)
}

/// Runs a plan that must succeed.
pub async fn require(
    runner: &dyn CommandRunner,
    command: &Command,
    timeout: Duration,
) -> Result<Output, StackError> {
    Ok(run_ok(runner, command, timeout).await?)
}
