//! The `cli` profile's client (spec §6.3): a mostro-cli build, the CLI's
//! offline discovery, and the adapter and sessions that drive it.
//!
//! A CLI build is a bundle of two files: the `mostro-cli` executable and
//! `mortsom-cli.json`, the daemon key and relays it is pointed at. The
//! bundle fingerprint pins both, as an app build pins the identity compiled
//! into it, so a recovery reopens the CLI against the same daemon.

use std::collections::BTreeSet;
use std::path::Path;
use std::time::Duration;

use mortsom_cli_driver::{CliProcess, Envelope, ProcessRequest};
use mortsom_desktop_driver::Platform;
use serde_json::Value;

/// The profile name.
pub const PROFILE: &str = mortsom_app_contract::config::CLI_PROFILE;
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
        let input = |command: &str, pointer: &str| {
            json.iter()
                .filter(|c| c.get("name").and_then(Value::as_str) == Some(command))
                .find_map(|c| c.pointer(pointer))
        };
        let order_expiry_in_seconds =
            input("order.new", "/input_schema/properties/expires-in").is_some();
        // The contract's sentinel, not merely the key: `"x-stdin": "-"`.
        let stdin_invoice = input(
            "order.add-invoice",
            "/input_schema/properties/invoice/x-stdin",
        )
        .and_then(Value::as_str)
            == Some("-");
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
    // The child runs in a temporary directory, where a relative path would
    // be looked up: resolve it against the caller's directory first.
    let program = binary
        .canonicalize()
        .map_err(|e| format!("mostro-cli executable {}: {e}", binary.display()))?;
    let dir = tempfile::tempdir().map_err(|e| e.to_string())?;
    let request = ProcessRequest {
        program,
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

#[cfg(test)]
#[path = "mostro_cli_tests.rs"]
mod tests;
