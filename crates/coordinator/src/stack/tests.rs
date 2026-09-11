#![allow(clippy::unwrap_used)]
//! The stack module against a scripted Docker: no daemon, no network.
use super::*;
use async_trait::async_trait;
use mortsom_ui_driver::process::{ChildProcess, Command, CommandRunner, Output, ProcessError};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

const IMAGE_ID: &str = "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const DIGEST: &str = "sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

/// A Docker that behaves like a regtest stack being built: nodes gain a
/// balance once funded and a channel once one is opened.
struct ScriptedStack {
    root: PathBuf,
    calls: Mutex<Vec<String>>,
    funded: Mutex<bool>,
    channels: Mutex<Vec<String>>,
    daemon_state: &'static str,
    running_image: &'static str,
    owner: Option<PathBuf>,
}

impl ScriptedStack {
    fn fresh(layout: &Layout) -> Self {
        Self {
            root: layout.root().to_path_buf(),
            calls: Mutex::new(Vec::new()),
            funded: Mutex::new(false),
            channels: Mutex::new(Vec::new()),
            daemon_state: "running",
            running_image: IMAGE_ID,
            owner: None,
        }
    }

    fn built(layout: &Layout) -> Self {
        let stack = Self::fresh(layout);
        *stack.funded.lock().unwrap() = true;
        *stack.channels.lock().unwrap() = vec!["lnd-alice".into(), "lnd-bob".into()];
        stack
    }

    fn calls(&self) -> Vec<String> {
        self.calls.lock().unwrap().clone()
    }

    fn matching(&self, needle: &str) -> Vec<String> {
        self.calls()
            .into_iter()
            .filter(|call| call.contains(needle))
            .collect()
    }

    fn reply(&self, key: &str, args: &[String]) -> Output {
        let has = |needle: &str| args.iter().any(|arg| arg == needle);
        let ok = |stdout: String| Output {
            success: true,
            stdout,
            stderr: String::new(),
        };
        if key.starts_with("docker ps") {
            return ok(self
                .owner
                .as_ref()
                .map(|_| "c".repeat(64))
                .unwrap_or_default());
        }
        if key.starts_with("docker inspect") {
            let format = args.iter().find(|arg| arg.contains("{{")).unwrap();
            if format.contains("working_dir") {
                let owner = self.owner.clone().unwrap_or_else(|| self.root.clone());
                return ok(serde_json::json!({
                    "working_dir": owner.display().to_string(),
                    "config_files": owner.join("compose.yml").display().to_string(),
                })
                .to_string());
            }
            return ok(serde_json::json!({
                "image": self.running_image,
                "running": self.daemon_state == "running",
            })
            .to_string());
        }
        if key.starts_with("docker image inspect") {
            return ok(serde_json::json!({
                "id": IMAGE_ID,
                "digests": [format!("mostrop2p/mostro@{DIGEST}")],
            })
            .to_string());
        }
        if has("getinfo") {
            return ok(r#"{"identity_pubkey":"02carol"}"#.into());
        }
        if has("newaddress") {
            return ok(r#"{"address":"bcrt1qnode"}"#.into());
        }
        if has("getnewaddress") {
            return ok("bcrt1qmine".into());
        }
        if has("sendtoaddress") {
            *self.funded.lock().unwrap() = true;
        }
        if has("walletbalance") {
            let sats = if *self.funded.lock().unwrap() {
                "99000"
            } else {
                "0"
            };
            return ok(format!(r#"{{"confirmed_balance":"{sats}"}}"#));
        }
        if has("openchannel") {
            self.channels.lock().unwrap().push(args[3].clone());
        }
        if has("listchannels") {
            let service = &args[3];
            let channels = self.channels.lock().unwrap();
            let count = if service == "lnd-carol" {
                channels.len()
            } else {
                channels.iter().filter(|c| *c == service).count()
            };
            let items: Vec<_> = (0..count)
                .map(|_| serde_json::json!({"active": true}))
                .collect();
            return ok(serde_json::json!({"channels": items}).to_string());
        }
        if has("ps") && has("-q") {
            return ok("d".repeat(64));
        }
        if has("ps") {
            return ok(format!(
                "bitcoind running\nlnd-alice running\nlnd-bob running\nlnd-carol running\nrelay running\nmostro {}\n",
                self.daemon_state
            ));
        }
        ok(String::new())
    }
}

#[async_trait]
impl CommandRunner for ScriptedStack {
    async fn run(&self, command: &Command, _timeout: Duration) -> Result<Output, ProcessError> {
        let key = format!("{} {}", command.program.display(), command.args.join(" "));
        self.calls.lock().unwrap().push(key.clone());
        Ok(self.reply(&key, &command.args))
    }

    async fn spawn(
        &self,
        _command: &Command,
        _log_file: Option<&Path>,
    ) -> Result<Box<dyn ChildProcess>, ProcessError> {
        unreachable!("the stack never spawns")
    }
}

struct Publishes(bool);

#[async_trait]
impl InfoProbe for Publishes {
    async fn daemon_published_info(&self, _relay_url: &str, _pubkey: PublicKey) -> bool {
        self.0
    }
}

fn scratch() -> (tempfile::TempDir, Layout) {
    let dir = tempfile::tempdir().unwrap();
    for name in ["compose.yml", "relay-config.toml"] {
        std::fs::write(dir.path().join(name), "").unwrap();
    }
    std::fs::write(
        dir.path().join("mostro-settings.tpl.toml"),
        include_str!("../../../../stack/mostro-settings.tpl.toml"),
    )
    .unwrap();
    std::fs::write(
        dir.path().join("limits.toml"),
        include_str!("../../../../stack/limits.toml"),
    )
    .unwrap();
    let layout = Layout::new(dir.path()).unwrap();
    (dir, layout)
}

fn quick() -> UpOptions {
    UpOptions {
        image: DEFAULT_MOSTRO_IMAGE.into(),
        timing: Timing {
            wait: Duration::ZERO,
            poll: Duration::ZERO,
        },
    }
}

#[test]
fn compose_commands_carry_project_file_and_invoking_user_in_the_environment() {
    // Arrange
    let (_dir, layout) = scratch();
    let compose = Compose::new(&layout, Some("mostrop2p/mostro@sha256:abc"));

    // Act
    let command = compose.command(&["ps"]);

    // Assert
    assert_eq!(command.program, Path::new("docker"));
    assert_eq!(command.args, ["compose", "ps"]);
    assert_eq!(
        command.env["COMPOSE_FILE"],
        layout.compose_file().display().to_string()
    );
    assert_eq!(command.env["COMPOSE_PROJECT_NAME"], PROJECT);
    assert_eq!(
        command.env["MORTSOM_MOSTRO_IMAGE"],
        "mostrop2p/mostro@sha256:abc"
    );
    for id in ["MORTSOM_UID", "MORTSOM_GID"] {
        assert!(
            command.env[id].chars().all(|c| c.is_ascii_digit()),
            "{id}={}",
            command.env[id]
        );
    }
}

#[tokio::test]
async fn up_mines_past_coinbase_maturity_before_funding_anything() {
    // Arrange
    let (_dir, layout) = scratch();
    let docker = ScriptedStack::fresh(&layout);

    // Act
    up(&docker, &Publishes(true), &layout, &quick())
        .await
        .unwrap();

    // Assert
    let mined: Vec<String> = docker
        .matching("generatetoaddress")
        .into_iter()
        .map(|call| call.split(' ').rev().nth(1).unwrap().to_owned())
        .collect();
    assert_eq!(mined, ["150", "6", "6"]);
    let first_funding = docker
        .calls()
        .iter()
        .position(|call| call.contains("sendtoaddress"))
        .unwrap();
    let first_mining = docker
        .calls()
        .iter()
        .position(|call| call.contains("generatetoaddress"))
        .unwrap();
    assert!(first_mining < first_funding);
}

#[tokio::test]
async fn up_opens_one_balanced_channel_per_actor_and_none_between_them() {
    // Arrange
    let (_dir, layout) = scratch();
    let docker = ScriptedStack::fresh(&layout);

    // Act
    up(&docker, &Publishes(true), &layout, &quick())
        .await
        .unwrap();

    // Assert
    let opens = docker.matching("openchannel");
    assert_eq!(opens.len(), 2, "{opens:?}");
    for call in &opens {
        assert!(call.contains("--node_key=02carol"), "{call}");
        assert!(call.contains("--local_amt=1000000"), "{call}");
        assert!(call.contains("--push_amt=500000"), "{call}");
    }
    assert!(opens[0].contains("exec -T lnd-alice "), "{}", opens[0]);
    assert!(opens[1].contains("exec -T lnd-bob "), "{}", opens[1]);
}

#[tokio::test]
async fn up_leaves_funded_wallets_and_existing_channels_alone() {
    // Arrange
    let (_dir, layout) = scratch();
    let docker = ScriptedStack::built(&layout);

    // Act
    up(&docker, &Publishes(true), &layout, &quick())
        .await
        .unwrap();

    // Assert
    assert!(docker.matching("openchannel").is_empty());
    assert!(docker.matching("sendtoaddress").is_empty());
}

#[tokio::test]
async fn up_mints_a_fresh_identity_into_owner_only_settings_and_a_public_descriptor() {
    // Arrange
    let (_dir, layout) = scratch();
    let docker = ScriptedStack::fresh(&layout);

    // Act
    let descriptor = up(&docker, &Publishes(true), &layout, &quick())
        .await
        .unwrap();

    // Assert
    let settings = std::fs::read_to_string(layout.settings_file()).unwrap();
    assert!(!settings.contains("__NSEC__"));
    assert!(settings.contains("nsec1"));
    use std::os::unix::fs::PermissionsExt;
    let mode = std::fs::metadata(layout.settings_file())
        .unwrap()
        .permissions()
        .mode();
    assert_eq!(mode & 0o777, 0o600);
    let public = std::fs::read_to_string(layout.descriptor_file()).unwrap();
    assert!(!public.contains("nsec"), "{public}");
    assert_eq!(descriptor.daemon_pubkey.len(), 64);
    assert!(public.contains(&descriptor.daemon_pubkey));
    assert_eq!(descriptor.daemon_image.image_id, IMAGE_ID);
    assert_eq!(
        descriptor.daemon_image.repository_digest.as_deref(),
        Some(DIGEST)
    );
    assert_eq!(descriptor.declared_limits.hold_invoice_expiry_secs, 120);
    // The daemon starts only after its settings exist, with the resolved image.
    let daemon_start = docker
        .calls()
        .iter()
        .position(|call| call.contains("up -d --force-recreate mostro"))
        .unwrap();
    let base_start = docker
        .calls()
        .iter()
        .position(|call| call.contains("up -d bitcoind"))
        .unwrap();
    assert!(base_start < daemon_start);
}

#[tokio::test]
async fn two_stacks_never_share_an_identity() {
    // Arrange
    let (_a, layout_a) = scratch();
    let (_b, layout_b) = scratch();

    // Act
    let a = up(
        &ScriptedStack::fresh(&layout_a),
        &Publishes(true),
        &layout_a,
        &quick(),
    )
    .await
    .unwrap();
    let b = up(
        &ScriptedStack::fresh(&layout_b),
        &Publishes(true),
        &layout_b,
        &quick(),
    )
    .await
    .unwrap();

    // Assert
    assert_ne!(a.daemon_pubkey, b.daemon_pubkey);
}

#[tokio::test]
async fn up_refuses_when_the_pull_fails_and_starts_nothing() {
    // Arrange
    let (_dir, layout) = scratch();
    let runner = mortsom_ui_driver::process::tests_support::FakeRunner::default();
    runner.script_failure("docker pull", "manifest unknown");

    // Act
    let error = up(&runner, &Publishes(true), &layout, &quick())
        .await
        .unwrap_err();

    // Assert
    assert!(error.to_string().contains("manifest unknown"), "{error}");
    assert!(
        runner
            .calls()
            .iter()
            .all(|call| !call.contains("compose up"))
    );
    assert!(!layout.local_dir().exists());
}

#[tokio::test]
async fn up_fails_when_the_daemon_never_publishes_its_info_event() {
    // Arrange
    let (_dir, layout) = scratch();
    let docker = ScriptedStack::fresh(&layout);

    // Act
    let error = up(&docker, &Publishes(false), &layout, &quick())
        .await
        .unwrap_err();

    // Assert
    assert!(error.to_string().contains("info event"), "{error}");
    assert!(docker.calls().iter().all(|call| !call.contains(" down")));
}

#[tokio::test]
async fn resolve_reads_only_public_facts_from_the_stack_it_owns() {
    // Arrange
    let (dir, layout) = scratch();
    let docker = ScriptedStack::fresh(&layout);
    up(&docker, &Publishes(true), &layout, &quick())
        .await
        .unwrap();

    // Act
    let context = StackContext::resolve(dir.path(), &docker).await.unwrap();

    // Assert
    assert_eq!(context.lookup("MORTSOM_MOSTRO_PUBKEY").unwrap().len(), 64);
    assert_eq!(
        context.lookup("MORTSOM_ALICE_LND_CERT").unwrap(),
        layout
            .root()
            .join("local/creds/alice/tls.cert")
            .display()
            .to_string()
    );
    assert_eq!(
        context.lookup("MORTSOM_BOB_LND_MACAROON").unwrap(),
        layout
            .root()
            .join("local/creds/bob/admin.macaroon")
            .display()
            .to_string()
    );
    assert_eq!(
        context.lookup("MORTSOM_ALICE_LND_HOST").unwrap(),
        "https://127.0.0.1:10001"
    );
    assert_eq!(
        context.lookup("MORTSOM_MOSTRO_SETTINGS_FILE").unwrap(),
        layout.settings_file().display().to_string()
    );
    let snapshot = context.snapshot();
    assert_eq!(snapshot.relay_url, "ws://127.0.0.1:7000");
    assert_eq!(snapshot.wallet_endpoints["bob"], "https://127.0.0.1:10002");
    assert!(context.lookup("MORTSOM_MOSTRO_NSEC").is_none());
}

#[tokio::test]
async fn resolve_refuses_a_directory_where_stack_up_never_ran() {
    // Arrange
    let (dir, layout) = scratch();
    let docker = ScriptedStack::fresh(&layout);

    // Act
    let error = StackContext::resolve(dir.path(), &docker)
        .await
        .unwrap_err();

    // Assert
    assert!(error.to_string().contains("stack up"), "{error}");
}

#[tokio::test]
async fn resolve_refuses_a_stack_whose_daemon_is_not_running_the_recorded_image() {
    // Arrange
    let (dir, layout) = scratch();
    let docker = ScriptedStack::fresh(&layout);
    up(&docker, &Publishes(true), &layout, &quick())
        .await
        .unwrap();
    let mut stale = ScriptedStack::built(&layout);
    stale.running_image = "sha256:cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc";
    let mut stopped = ScriptedStack::built(&layout);
    stopped.daemon_state = "exited";

    // Act / Assert
    for docker in [stale, stopped] {
        assert!(StackContext::resolve(dir.path(), &docker).await.is_err());
    }
}

#[tokio::test]
async fn status_reports_every_service_node_and_the_identity_in_effect() {
    // Arrange
    let (_dir, layout) = scratch();
    let docker = ScriptedStack::fresh(&layout);
    let descriptor = up(&docker, &Publishes(true), &layout, &quick())
        .await
        .unwrap();

    // Act
    let report = status(&docker, &layout).await.unwrap();

    // Assert
    assert!(report.is_ready());
    let rendered = report.render();
    for service in [
        "bitcoind",
        "lnd-alice",
        "lnd-bob",
        "lnd-carol",
        "relay",
        "mostro",
    ] {
        assert!(rendered.contains(service), "{rendered}");
    }
    assert!(rendered.contains(&descriptor.daemon_pubkey), "{rendered}");
    assert!(rendered.contains("ready"), "{rendered}");
}

#[tokio::test]
async fn status_is_not_ready_before_stack_up_or_with_a_node_lacking_a_channel() {
    // Arrange
    let (_dir, layout) = scratch();
    let never_built = ScriptedStack::fresh(&layout);

    // Act
    let report = status(&never_built, &layout).await.unwrap();

    // Assert
    assert!(!report.is_ready());
    assert!(report.render().contains("not ready"), "{}", report.render());
}

#[tokio::test]
async fn down_removes_services_volumes_and_every_generated_secret() {
    // Arrange
    let (_dir, layout) = scratch();
    let docker = ScriptedStack::fresh(&layout);
    up(&docker, &Publishes(true), &layout, &quick())
        .await
        .unwrap();
    assert!(layout.local_dir().exists());

    // Act
    down(&docker, &layout).await.unwrap();

    // Assert
    assert!(
        docker
            .calls()
            .iter()
            .any(|call| call.ends_with("compose --profile daemon down -v --remove-orphans")),
        "{:?}",
        docker.calls()
    );
    assert!(!layout.local_dir().exists());
}

#[tokio::test]
async fn up_and_down_refuse_containers_another_directory_created() {
    // Arrange
    let (_dir, layout) = scratch();
    let mut docker = ScriptedStack::built(&layout);
    docker.owner = Some(PathBuf::from("/elsewhere/stack"));

    // Act
    let up_error = up(&docker, &Publishes(true), &layout, &quick())
        .await
        .unwrap_err();
    let down_error = down(&docker, &layout).await.unwrap_err();

    // Assert
    for error in [up_error, down_error] {
        assert!(error.to_string().contains("ownership"), "{error}");
    }
    assert!(docker.calls().iter().all(|call| !call.contains(" down")));
    assert!(
        docker
            .calls()
            .iter()
            .all(|call| !call.contains("compose up"))
    );
}

/// What the payment-failure family drives comes from the declared limits,
/// undeclared caps staying undeclared rather than defaulting.
#[test]
fn payout_limits_are_the_declared_ones_or_nothing() {
    let mut limits: DeclaredLimits =
        toml::from_str(include_str!("../../../../stack/limits.toml")).unwrap();
    let payout = limits.payout_limits();
    assert_eq!(payout.payout_invoice_min_expiry_secs, Some(300));
    assert_eq!(payout.max_inflight_payouts_per_destination, Some(2));
    limits.max_inflight_payouts_per_destination = None;
    assert_eq!(
        limits.payout_limits().max_inflight_payouts_per_destination,
        None
    );
}

#[test]
fn declared_limits_match_the_settings_template() {
    // Arrange
    let limits: DeclaredLimits =
        toml::from_str(include_str!("../../../../stack/limits.toml")).unwrap();
    let template: toml::Table = include_str!("../../../../stack/mostro-settings.tpl.toml")
        .parse()
        .unwrap();
    let value = |section: &str, key: &str| template[section][key].as_integer().unwrap() as u64;

    // Assert
    assert_eq!(
        limits.hold_invoice_expiry_secs,
        value("lightning", "hold_invoice_expiration_window")
    );
    assert_eq!(
        limits.payout_invoice_min_expiry_secs,
        value("lightning", "invoice_expiration_window")
    );
    assert_eq!(
        limits.buyer_invoice_deadline_secs,
        value("mostro", "expiration_seconds")
    );
    assert_eq!(
        limits.max_inflight_payouts_per_destination,
        Some(value("lightning", "max_inflight_payouts_per_destination") as u32)
    );
    assert!(limits.validate().is_ok());
}

#[test]
fn the_compose_file_publishes_exactly_the_ports_the_spec_fixes() {
    // Arrange
    let compose = include_str!("../../../../stack/compose.yml");

    // Assert
    for published in [
        "\"10001:10009\"",
        "\"10002:10009\"",
        "\"10003:10009\"",
        "\"127.0.0.1:7000:7000\"",
    ] {
        assert!(compose.contains(published), "{published}");
    }
    assert_eq!(
        compose.matches("ports:").count(),
        4,
        "bitcoind and mostro publish nothing"
    );
    assert!(compose.contains(&format!("MORTSOM_MOSTRO_IMAGE:-{DEFAULT_MOSTRO_IMAGE}")));
    assert!(compose.contains(&format!("name: {PROJECT}")));
}

#[test]
fn credential_references_live_under_the_stack_directory() {
    // Act / Assert
    assert_eq!(
        credential_reference(Path::new("/s"), "MORTSOM_ALICE_LND_CERT").unwrap(),
        Path::new("/s/local/creds/alice/tls.cert")
    );
    assert_eq!(
        credential_reference(Path::new("/s"), "MORTSOM_BOB_LND_MACAROON").unwrap(),
        Path::new("/s/local/creds/bob/admin.macaroon")
    );
    assert!(credential_reference(Path::new("/s"), "MORTSOM_CAROL_LND_CERT").is_none());
    assert!(credential_reference(Path::new("/s"), "MORTSOM_MOSTRO_PUBKEY").is_none());
}
