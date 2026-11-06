//! The `cli` profile's client (spec §6.3): a mostro-cli build, the CLI's
//! offline discovery, and the adapter and sessions that drive it.
//!
//! A CLI build is a bundle of two files: the `mostro-cli` executable and
//! `mortsom-cli.json`, the daemon key and relays it is pointed at. The
//! bundle fingerprint pins both, as an app build pins the identity compiled
//! into it, so a recovery reopens the CLI against the same daemon.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use mortsom_adapter_cli::{CliAdapter, CliAdapterConfig};
use mortsom_app_contract::AppAdapter;
use mortsom_cli_driver::{
    CliClient, CliConfig, CliProcess, CliSession, Envelope, NodeTarget, ProcessRequest,
    TokioCliProcess,
};
use mortsom_desktop_driver::Platform;
use mortsom_desktop_driver::storage::ActorStorage;
use mortsom_ui_driver::UiSession;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::Config;
use crate::provenance::BuildProvenance;
use crate::run::DesktopOptions;

/// The profile name.
pub const PROFILE: &str = "cli";
/// The executable's name inside a CLI build.
pub const EXECUTABLE: &str = "mostro-cli";
/// The node file's name inside a CLI build.
pub const NODE_FILE: &str = "mortsom-cli.json";
/// Deadline of one CLI invocation, longer than the CLI's own deadlines
/// (connection, probe, reply and cleanup add up to about 29 seconds).
pub const INVOCATION_TIMEOUT: Duration = Duration::from_secs(90);
/// How long the CLI waits for a node reply when it can be told
/// (`--reply-timeout`): longer than the CLI's 15-second default, for a
/// daemon slowed by its own database contention.
pub const REPLY_TIMEOUT_SECS: u64 = 60;
/// Deadline of the offline discovery.
const DISCOVERY_TIMEOUT: Duration = Duration::from_secs(30);

/// Commands the profile cannot run without; the rest only enable
/// capabilities.
pub const REQUIRED_COMMANDS: &[&str] = &[
    "identity.init",
    "identity.show",
    "order.list",
    "order.new",
    "order.info",
    "order.take-sell",
    "order.take-buy",
    "order.add-invoice",
    "order.fiat-sent",
    "order.release",
    "order.cancel",
    "message.list",
];

/// The client a run drives: the configured profile unless `--profile`
/// overrides it, and the runtime that profile runs on. `--platform` picks
/// an app platform, so it is refused with the CLI.
pub fn resolve_client(
    configured: &str,
    profile: Option<&str>,
    platform: Option<Platform>,
) -> Result<(String, Platform), String> {
    let profile = profile.unwrap_or(configured);
    match profile {
        PROFILE if platform.is_some() => Err(
            "--platform selects an app platform; profile `cli` drives mostro-cli and takes none"
                .into(),
        ),
        PROFILE => Ok((PROFILE.into(), Platform::Cli)),
        mortsom_app_contract::config::DEFAULT_PROFILE => {
            Ok((profile.into(), platform.unwrap_or_default()))
        }
        other => Err(format!(
            "unknown profile `{other}` (expected one of {})",
            mortsom_app_contract::config::KNOWN_PROFILES.join(", ")
        )),
    }
}

/// The daemon and relays a CLI build is pointed at.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NodeFile {
    /// Daemon public key, as configured.
    pub mostro_pubkey: String,
    /// The run relay, reached from this host.
    pub relays: Vec<String>,
    /// The reply budget passed to the CLI, when the build offers one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reply_timeout_secs: Option<u64>,
}

impl NodeFile {
    /// The daemon and relay of `config`, as an app build embeds them, and
    /// the reply budget when the CLI takes one.
    pub fn for_config(config: &Config, discovery: &Discovery) -> Self {
        Self {
            mostro_pubkey: config.mobile.mostro_pubkey.clone(),
            relays: vec![format!("ws://127.0.0.1:{}", config.relay.host_port)],
            reply_timeout_secs: discovery.reply_timeout.then_some(REPLY_TIMEOUT_SECS),
        }
    }

    /// Reads the node file of the build in `bundle`.
    pub fn read(bundle: &Path) -> Result<Self, String> {
        let text = std::fs::read_to_string(bundle.join(NODE_FILE))
            .map_err(|e| format!("cannot read the CLI build's {NODE_FILE}: {e}"))?;
        serde_json::from_str(&text).map_err(|e| format!("invalid {NODE_FILE}: {e}"))
    }

    fn target(&self) -> NodeTarget {
        NodeTarget {
            mostro_pubkey: self.mostro_pubkey.clone(),
            relays: self.relays.clone(),
            reply_timeout_secs: self.reply_timeout_secs,
        }
    }
}

/// What the built CLI says it can do (`mostro-cli schema`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Discovery {
    /// The executable's version.
    pub version: String,
    /// Commands with a JSON output mode, by dotted name.
    pub commands: BTreeSet<String>,
    /// Whether `order new` takes a lifetime in seconds (`--expires-in`).
    pub order_expiry_in_seconds: bool,
    /// Whether `order add-invoice` reads `--invoice -` from standard input.
    pub stdin_invoice: bool,
    /// Whether the CLI takes a reply budget (`--reply-timeout`).
    pub reply_timeout: bool,
}

impl Discovery {
    /// Parses the discovery document.
    pub fn parse(stdout: &str, exit: Option<i32>) -> Result<Self, String> {
        let envelope = Envelope::parse(stdout, exit)?;
        if envelope.command != "schema" {
            return Err(format!("discovery answered as `{}`", envelope.command));
        }
        let commands = envelope
            .field(&["commands"])
            .and_then(Value::as_array)
            .ok_or("discovery lists no commands")?;
        let json: Vec<&Value> = commands
            .iter()
            .filter(|c| {
                c.get("output_modes")
                    .and_then(Value::as_array)
                    .is_some_and(|modes| modes.iter().any(|m| m == "json"))
            })
            .collect();
        let has_input = |command: &str, pointer: &str| {
            json.iter().any(|c| {
                c.get("name").and_then(Value::as_str) == Some(command)
                    && c.pointer(pointer).is_some()
            })
        };
        let order_expiry_in_seconds = has_input("order.new", "/input_schema/properties/expires-in");
        let stdin_invoice = has_input(
            "order.add-invoice",
            "/input_schema/properties/invoice/x-stdin",
        );
        let reply_timeout = envelope
            .field(&["global_options"])
            .and_then(Value::as_array)
            .is_some_and(|options| {
                options
                    .iter()
                    .any(|o| o.get("name").and_then(Value::as_str) == Some("reply-timeout"))
            });
        Ok(Self {
            stdin_invoice,
            reply_timeout,
            version: envelope
                .text(&["executable_version"])
                .unwrap_or("")
                .to_owned(),
            commands: json
                .iter()
                .filter_map(|c| c.get("name").and_then(Value::as_str).map(str::to_owned))
                .collect(),
            order_expiry_in_seconds,
        })
    }

    /// Required commands the CLI lacks.
    pub fn missing(&self) -> Vec<&'static str> {
        REQUIRED_COMMANDS
            .iter()
            .copied()
            .filter(|command| !self.commands.contains(*command))
            .collect()
    }
}

/// Runs the executable's offline discovery: no network, no identity, an
/// emptied environment and a private working directory.
pub async fn discover(binary: &Path, process: &dyn CliProcess) -> Result<Discovery, String> {
    let dir = tempfile::tempdir().map_err(|e| e.to_string())?;
    let request = ProcessRequest {
        program: binary.to_path_buf(),
        args: vec!["--output".into(), "json".into(), "schema".into()],
        env: Default::default(),
        current_dir: dir.path().to_path_buf(),
        stdin: None,
    };
    let output = process
        .run(&request, DISCOVERY_TIMEOUT)
        .await
        .map_err(|e| format!("mostro-cli discovery: {e}"))?;
    Discovery::parse(&output.stdout, output.code).map_err(|e| format!("mostro-cli discovery: {e}"))
}

/// Refuses a CLI that lacks a required command, naming them, or that would
/// need an invoice in its arguments (spec §10).
pub fn require(discovery: &Discovery) -> Result<(), String> {
    let missing = discovery.missing();
    if !missing.is_empty() {
        return Err(format!(
            "mostro-cli {} lacks JSON commands the cli profile needs: {}",
            discovery.version,
            missing.join(", ")
        ));
    }
    if !discovery.stdin_invoice {
        return Err(format!(
            "mostro-cli {} cannot read an invoice from standard input (`--invoice -`)",
            discovery.version
        ));
    }
    Ok(())
}

/// Makes `executable` the run's CLI build, pointed at `config`'s daemon.
pub async fn prepare(
    config: &Config,
    executable: &Path,
    provenance: BuildProvenance,
) -> Result<DesktopOptions, String> {
    let staging = tempfile::Builder::new()
        .prefix("mortsom-cli-build-")
        .tempdir()
        .map_err(|e| e.to_string())?;
    let discovery = discover(executable, &TokioCliProcess).await?;
    require(&discovery)?;
    stage(
        staging.path(),
        executable,
        &NodeFile::for_config(config, &discovery),
    )?;
    let root = staging.path().to_path_buf();
    let bundle =
        tokio::task::spawn_blocking(move || crate::fingerprint::CapturedBundle::capture(&root))
            .await
            .map_err(|_| "CLI build capture task failed".to_string())?
            .map_err(|e| format!("cannot capture the CLI build: {e}"))?;
    let artifact = bundle.root().join(EXECUTABLE);
    Ok(DesktopOptions {
        recovery: None,
        resume: false,
        platform: Platform::Cli,
        artifact,
        bundle: Arc::new(bundle),
        provenance,
        xvfb: PathBuf::new(),
        chromedriver: PathBuf::new(),
        browser: PathBuf::new(),
        relay_audit: None,
    })
}

/// Copies the executable and writes the node file into `dir`.
fn stage(dir: &Path, executable: &Path, node: &NodeFile) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    validate_executable(executable)?;
    let target = dir.join(EXECUTABLE);
    std::fs::copy(executable, &target).map_err(|e| format!("cannot stage mostro-cli: {e}"))?;
    std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o755))
        .map_err(|e| e.to_string())?;
    let text = serde_json::to_string_pretty(node).map_err(|e| e.to_string())?;
    std::fs::write(dir.join(NODE_FILE), text).map_err(|e| e.to_string())
}

/// A regular, executable file.
pub fn validate_executable(path: &Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    let metadata = std::fs::metadata(path)
        .map_err(|e| format!("mostro-cli executable {}: {e}", path.display()))?;
    if !metadata.is_file() || metadata.permissions().mode() & 0o111 == 0 {
        return Err(format!(
            "mostro-cli executable {} is not an executable file",
            path.display()
        ));
    }
    Ok(())
}

/// A captured CLI build: its executable and node file.
pub fn validate_bundle(artifact: &Path) -> Result<(), String> {
    validate_executable(artifact)?;
    let bundle = artifact.parent().ok_or("CLI build has no directory")?;
    NodeFile::read(bundle).map(|_| ())
}

/// The adapter for the CLI in `options`, with what its discovery offers.
pub async fn adapter_for(
    options: &DesktopOptions,
    config: &Config,
) -> Result<Arc<dyn AppAdapter>, String> {
    let discovery = discover(&options.artifact, &TokioCliProcess).await?;
    require(&discovery)?;
    Ok(Arc::new(CliAdapter::new(CliAdapterConfig {
        order_expiry_secs: config.mobile.order_expiry_secs,
        commands: discovery.commands,
        order_expiry_in_seconds: discovery.order_expiry_in_seconds,
    })))
}

/// One actor's CLI session, in retained storage when the run records one.
pub fn session(
    options: &DesktopOptions,
    storage: Option<ActorStorage>,
) -> Result<Arc<dyn UiSession>, String> {
    let node = NodeFile::read(options.bundle.root())?;
    let config = CliConfig {
        binary: options.artifact.clone(),
        node: node.target(),
        timeout: INVOCATION_TIMEOUT,
        path_env: None,
    };
    let client = CliClient::new(config, Arc::new(TokioCliProcess));
    let client = match storage {
        Some(storage) => client.with_storage(storage),
        None => client,
    };
    Ok(Arc::new(CliSession::new(Arc::new(client))))
}

#[cfg(test)]
#[path = "mostro_cli_tests.rs"]
mod tests;
