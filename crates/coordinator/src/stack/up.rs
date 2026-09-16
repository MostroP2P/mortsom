//! `stack up`: builds the whole world from nothing, skipping what exists.
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use mortsom_ui_driver::process::CommandRunner;
use nostr_sdk::prelude::{Keys, ToBech32};

use super::compose::{
    BASE_SERVICES, Compose, DAEMON_SERVICE, LND_DIR, MINER_WALLET, attempt, require,
};
use super::descriptor::{Descriptor, load_limits};
use super::layout::Layout;
use super::metadata::DaemonImage;
use super::{
    DaemonSource, InfoProbe, StackError, connections::Connections, daemon, image, nodes, ownership,
};

/// Channel each actor opens to the escrow node, and what it pushes across,
/// so both sides can pay a hold and receive a payout (spec section 5).
pub const CHANNEL_SATS: u64 = 1_000_000;
pub const PUSH_SATS: u64 = 500_000;
/// Past coinbase maturity (100) with room for every funding transaction.
const INITIAL_BLOCKS: u32 = 150;
const CONFIRM_BLOCKS: u32 = 6;
/// The two actors; never the escrow node.
pub const ACTORS: [&str; 2] = ["alice", "bob"];
pub const ESCROW_NODE: &str = "carol";
const ALL_NODES: [&str; 3] = ["alice", "bob", "carol"];

const START_TIMEOUT: Duration = Duration::from_secs(600);
const STEP_TIMEOUT: Duration = Duration::from_secs(120);
const NSEC_PLACEHOLDER: &str = "__NSEC__";

/// How long to wait for each service and how often to look.
#[derive(Debug, Clone, Copy)]
pub struct Timing {
    /// Ceiling for each wait.
    pub wait: Duration,
    /// Interval between checks.
    pub poll: Duration,
}

impl Default for Timing {
    fn default() -> Self {
        Self {
            wait: Duration::from_secs(180),
            poll: Duration::from_secs(2),
        }
    }
}

/// What `up` needs beyond the directory.
#[derive(Debug, Clone)]
pub struct UpOptions {
    /// Which daemon to run.
    pub daemon: DaemonSource,
    /// Object cache and disposable checkouts for [`DaemonSource::Ref`].
    pub source_cache: PathBuf,
    /// Service wait policy.
    pub timing: Timing,
    /// Remove a project this directory cannot be shown to own -- another
    /// checkout's, or resources left without containers -- before building.
    pub replace: bool,
}

/// Builds or completes the stack and returns its public descriptor.
pub async fn up(
    runner: &dyn CommandRunner,
    probe: &dyn InfoProbe,
    layout: &Layout,
    options: &UpOptions,
) -> Result<Descriptor, StackError> {
    claim(runner, layout, options.replace).await?;
    let template = read(&layout.template())?;
    if !template.contains(NSEC_PLACEHOLDER) {
        return Err(StackError::Configuration(format!(
            "{} has no {NSEC_PLACEHOLDER} placeholder; the daemon would start without an identity",
            layout.template().display()
        )));
    }
    let limits = load_limits(&layout.limits_file())?;

    let (expected, provenance) =
        daemon::prepare(runner, &options.daemon, &options.source_cache).await?;
    let compose = Compose::new(layout, Some(&expected.reference));

    tracing::info!("starting chain, nodes and relay");
    let mut args = vec!["up", "-d"];
    args.extend(BASE_SERVICES);
    require(runner, &compose.command(&args), START_TIMEOUT).await?;

    let miner = prepare_chain(runner, &compose, options.timing).await?;
    fund_nodes(runner, &compose, options.timing, &miner).await?;
    open_channels(runner, &compose, options.timing, &miner).await?;
    copy_credentials(runner, &compose, layout).await?;

    let keys = Keys::generate();
    write_settings(layout, &template, &keys)?;
    start_daemon(runner, &compose, options.timing).await?;

    let descriptor = Descriptor::new(
        keys.public_key().to_hex(),
        Connections::for_stack(layout.root()),
        DaemonImage {
            image_id: expected.id.clone(),
            reference: expected.reference.clone(),
            repository_digest: expected
                .reference
                .rsplit_once('@')
                .map(|(_, digest)| digest.to_owned()),
        },
        provenance,
        limits,
    )?;
    descriptor.write(layout)?;

    tracing::info!("waiting for the daemon to publish its info event");
    let relay_url = descriptor.connections.relay_url.clone();
    let pubkey = keys.public_key();
    wait_for(options.timing, "the daemon's info event", || async {
        Ok(probe.daemon_published_info(&relay_url, pubkey).await)
    })
    .await?;
    image::verify_running(runner, &compose, layout, &expected).await?;
    Ok(descriptor)
}

/// Refuses a project this directory does not own; with `replace`, removes
/// it by name instead and checks again. A stack this directory owns is left
/// alone either way, since `up` completes it.
async fn claim(
    runner: &dyn CommandRunner,
    layout: &Layout,
    replace: bool,
) -> Result<(), StackError> {
    let replaced = match ownership::verify(runner, layout).await {
        Err(StackError::ForeignOwner(dir)) if replace => {
            format!("the stack {dir} created; the generated state there is stale from now on")
        }
        Err(StackError::Unowned(reason)) if replace => {
            format!("a stack no directory can be shown to own ({reason})")
        }
        outcome => return outcome,
    };
    tracing::warn!("--replace: removing {replaced}");
    ownership::release(runner, layout).await?;
    ownership::verify(runner, layout).await
}

async fn prepare_chain(
    runner: &dyn CommandRunner,
    compose: &Compose,
    timing: Timing,
) -> Result<String, StackError> {
    wait_for(timing, "bitcoind RPC", || async {
        Ok(
            attempt(runner, &compose.bitcoin_cli(&["getblockchaininfo"]))
                .await?
                .success,
        )
    })
    .await?;
    // Both fail harmlessly on a rerun: the wallet exists and is loaded.
    attempt(
        runner,
        &compose.bitcoin_cli(&["createwallet", MINER_WALLET]),
    )
    .await?;
    attempt(runner, &compose.bitcoin_cli(&["loadwallet", MINER_WALLET])).await?;
    let wallet = format!("-rpcwallet={MINER_WALLET}");
    let miner = require(
        runner,
        &compose.bitcoin_cli(&[&wallet, "getnewaddress"]),
        STEP_TIMEOUT,
    )
    .await?
    .stdout
    .trim()
    .to_owned();
    tracing::info!(blocks = INITIAL_BLOCKS, "mining the initial chain");
    mine(runner, compose, &miner, INITIAL_BLOCKS).await?;
    for node in ALL_NODES {
        wait_for(timing, &format!("lnd-{node} RPC"), || async {
            Ok(attempt(runner, &compose.lncli(node, &["getinfo"]))
                .await?
                .success)
        })
        .await?;
    }
    Ok(miner)
}

async fn fund_nodes(
    runner: &dyn CommandRunner,
    compose: &Compose,
    timing: Timing,
    miner: &str,
) -> Result<(), StackError> {
    let wallet = format!("-rpcwallet={MINER_WALLET}");
    let mut funded_now = false;
    for node in ALL_NODES {
        if confirmed_sats(runner, compose, node).await? > 0 {
            tracing::info!("lnd-{node} already holds funds");
            continue;
        }
        tracing::info!("funding lnd-{node}");
        let reply = require(
            runner,
            &compose.lncli(node, &["newaddress", "p2wkh"]),
            STEP_TIMEOUT,
        )
        .await?;
        let address = nodes::new_address(&reply.stdout)?;
        require(
            runner,
            &compose.bitcoin_cli(&[&wallet, "sendtoaddress", &address, "1"]),
            STEP_TIMEOUT,
        )
        .await?;
        funded_now = true;
    }
    if funded_now {
        mine(runner, compose, miner, CONFIRM_BLOCKS).await?;
    }
    for node in ALL_NODES {
        wait_for(timing, &format!("lnd-{node} confirmed funds"), || async {
            Ok(confirmed_sats(runner, compose, node).await? > 0)
        })
        .await?;
    }
    Ok(())
}

async fn confirmed_sats(
    runner: &dyn CommandRunner,
    compose: &Compose,
    node: &str,
) -> Result<u64, StackError> {
    let reply = attempt(runner, &compose.lncli(node, &["walletbalance"])).await?;
    if !reply.success {
        return Ok(0);
    }
    nodes::confirmed_balance(&reply.stdout)
}

async fn open_channels(
    runner: &dyn CommandRunner,
    compose: &Compose,
    timing: Timing,
    miner: &str,
) -> Result<(), StackError> {
    let reply = require(
        runner,
        &compose.lncli(ESCROW_NODE, &["getinfo"]),
        STEP_TIMEOUT,
    )
    .await?;
    let carol = nodes::identity_pubkey(&reply.stdout)?;
    let node_key = format!("--node_key={carol}");
    let local_amt = format!("--local_amt={CHANNEL_SATS}");
    let push_amt = format!("--push_amt={PUSH_SATS}");
    let peer = format!("{carol}@lnd-{ESCROW_NODE}:9735");
    let mut opened_now = false;
    for actor in ACTORS {
        let reply = require(
            runner,
            &compose.lncli(actor, &["listchannels"]),
            STEP_TIMEOUT,
        )
        .await?;
        if nodes::channel_count(&reply.stdout)? > 0 {
            tracing::info!("lnd-{actor} already has its channel");
            continue;
        }
        tracing::info!("opening lnd-{actor} to lnd-{ESCROW_NODE}");
        attempt(runner, &compose.lncli(actor, &["connect", &peer])).await?;
        require(
            runner,
            &compose.lncli(actor, &["openchannel", &node_key, &local_amt, &push_amt]),
            STEP_TIMEOUT,
        )
        .await?;
        opened_now = true;
    }
    if opened_now {
        mine(runner, compose, miner, CONFIRM_BLOCKS).await?;
    }
    for node in ALL_NODES {
        wait_for(timing, &format!("lnd-{node} active channel"), || async {
            let reply = attempt(runner, &compose.lncli(node, &["listchannels"])).await?;
            if !reply.success {
                return Ok(false);
            }
            Ok(nodes::active_channel_count(&reply.stdout)? > 0)
        })
        .await?;
    }
    Ok(())
}

/// Copies each node's certificate and admin macaroon out of its container:
/// the actors' into `local/creds`, carol's into the daemon's config. Wipes
/// the previous generation first, database included, so every `up` starts
/// the daemon with a fresh identity and no stale orders.
async fn copy_credentials(
    runner: &dyn CommandRunner,
    compose: &Compose,
    layout: &Layout,
) -> Result<(), StackError> {
    tracing::info!("copying node credentials");
    attempt(
        runner,
        &compose.command(&["--profile", "daemon", "rm", "-sf", DAEMON_SERVICE]),
    )
    .await?;
    remove_dir(&layout.local_dir())?;
    create_dir(&layout.daemon_lnd_dir())?;
    let cert = format!("{LND_DIR}/tls.cert");
    let macaroon = format!("{LND_DIR}/data/chain/bitcoin/regtest/admin.macaroon");
    for actor in ACTORS {
        let target = layout.creds_dir().join(actor);
        create_dir(&target)?;
        copy_out(runner, compose, actor, &cert, &target.join("tls.cert")).await?;
        copy_out(
            runner,
            compose,
            actor,
            &macaroon,
            &target.join("admin.macaroon"),
        )
        .await?;
    }
    let target = layout.daemon_lnd_dir();
    copy_out(
        runner,
        compose,
        ESCROW_NODE,
        &cert,
        &target.join("tls.cert"),
    )
    .await?;
    copy_out(
        runner,
        compose,
        ESCROW_NODE,
        &macaroon,
        &target.join("admin.macaroon"),
    )
    .await?;
    Ok(())
}

async fn copy_out(
    runner: &dyn CommandRunner,
    compose: &Compose,
    node: &str,
    from: &str,
    to: &Path,
) -> Result<(), StackError> {
    let source = format!("lnd-{node}:{from}");
    let target = to.display().to_string();
    require(
        runner,
        &compose.command(&["cp", &source, &target]),
        STEP_TIMEOUT,
    )
    .await?;
    // `docker compose cp` keeps the container's mode, which leaves the
    // macaroon group-readable; credentials here are owner-only.
    if to.is_file() {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(to, std::fs::Permissions::from_mode(0o600)).map_err(|error| {
            StackError::Configuration(format!("cannot restrict {}: {error}", to.display()))
        })?;
    }
    Ok(())
}

fn write_settings(layout: &Layout, template: &str, keys: &Keys) -> Result<(), StackError> {
    let nsec = keys.secret_key().to_bech32().map_err(|error| {
        StackError::Configuration(format!("cannot encode the daemon key: {error}"))
    })?;
    write_private(
        &layout.settings_file(),
        &template.replace(NSEC_PLACEHOLDER, &nsec),
    )
}

/// Creates or truncates `path` readable by its owner only, tightening a file
/// that already existed with wider permissions.
fn write_private(path: &Path, contents: &str) -> Result<(), StackError> {
    use std::io::Write as _;
    use std::os::unix::fs::{OpenOptionsExt as _, PermissionsExt as _};
    let cannot = |verb: &str, error: std::io::Error| {
        StackError::Configuration(format!("cannot {verb} {}: {error}", path.display()))
    };
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)
        .map_err(|error| cannot("write", error))?;
    file.write_all(contents.as_bytes())
        .map_err(|error| cannot("write", error))?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
        .map_err(|error| cannot("restrict", error))
}

async fn start_daemon(
    runner: &dyn CommandRunner,
    compose: &Compose,
    timing: Timing,
) -> Result<(), StackError> {
    tracing::info!("starting mostrod");
    require(
        runner,
        &compose.command(&[
            "--profile",
            "daemon",
            "up",
            "-d",
            "--force-recreate",
            DAEMON_SERVICE,
        ]),
        START_TIMEOUT,
    )
    .await?;
    wait_for(timing, "mostrod to come up", || async {
        let table = attempt(
            runner,
            &compose.command(&[
                "--profile",
                "daemon",
                "ps",
                "--format",
                "{{.Service}} {{.State}}",
            ]),
        )
        .await?;
        let running = table.success
            && nodes::parse_services(&table.stdout)
                .iter()
                .any(|service| service.name == DAEMON_SERVICE && service.is_running());
        if !running {
            return Ok(false);
        }
        Ok(attempt(
            runner,
            &compose.command(&[
                "exec",
                "-T",
                DAEMON_SERVICE,
                "test",
                "-f",
                "/config/mostro.db",
            ]),
        )
        .await?
        .success)
    })
    .await
}

async fn mine(
    runner: &dyn CommandRunner,
    compose: &Compose,
    miner: &str,
    blocks: u32,
) -> Result<(), StackError> {
    let wallet = format!("-rpcwallet={MINER_WALLET}");
    let blocks = blocks.to_string();
    require(
        runner,
        &compose.bitcoin_cli(&[&wallet, "generatetoaddress", &blocks, miner]),
        STEP_TIMEOUT,
    )
    .await?;
    Ok(())
}

async fn wait_for<F, Fut>(timing: Timing, what: &str, mut check: F) -> Result<(), StackError>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<bool, StackError>>,
{
    let deadline = Instant::now() + timing.wait;
    loop {
        if check().await? {
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err(StackError::Configuration(format!(
                "waited {}s for {what}; `docker compose logs` in the stack directory says why",
                timing.wait.as_secs()
            )));
        }
        tokio::time::sleep(timing.poll).await;
    }
}

fn read(path: &Path) -> Result<String, StackError> {
    std::fs::read_to_string(path).map_err(|error| {
        StackError::Configuration(format!("cannot read {}: {error}", path.display()))
    })
}

pub(super) fn remove_dir(path: &Path) -> Result<(), StackError> {
    match std::fs::remove_dir_all(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(StackError::Configuration(format!(
            "cannot remove {}: {error}",
            path.display()
        ))),
    }
}

fn create_dir(path: &Path) -> Result<(), StackError> {
    std::fs::create_dir_all(path).map_err(|error| {
        StackError::Configuration(format!("cannot create {}: {error}", path.display()))
    })
}
