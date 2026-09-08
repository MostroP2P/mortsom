#![allow(clippy::unwrap_used)]
use super::*;
use mortsom_android_driver::process::tests_support::FakeRunner;

#[test]
fn stack_command_uses_only_the_external_cli_and_explicit_checkout() {
    let cmd = stack_command(
        Path::new("/work/ortsom"),
        Path::new("/bin/ortsom"),
        "status",
    );
    assert_eq!(cmd.program, Path::new("env"));
    assert_eq!(
        cmd.args,
        [
            "--chdir=/work/ortsom",
            "COMPOSE_PROJECT_NAME=ortsom-regtest",
            "/bin/ortsom",
            "stack",
            "status"
        ]
    );
}

#[tokio::test]
async fn failed_stack_does_not_attempt_automatic_destruction() {
    let root = tempfile::tempdir().unwrap();
    let runner = FakeRunner::default();
    runner.script_failure("env", "stack failed");
    runner.script(
        "docker image inspect",
        &serde_json::json!({
            "id": format!("sha256:{}", "a".repeat(64)),
            "digests": [format!("mostrop2p/mostro@sha256:{}", "b".repeat(64))],
        })
        .to_string(),
    );
    assert!(
        invoke(&runner, root.path(), Path::new("ortsom"), "up")
            .await
            .is_err()
    );
    assert_eq!(runner.calls().len(), 6);
    assert!(runner.calls().iter().all(|c| !c.contains("down")));
}

#[tokio::test]
async fn imported_stack_uses_public_cli_without_opening_private_config() {
    let dir = tempfile::tempdir().unwrap();
    // A directory at the old filename makes any attempt to read it fail.
    std::fs::create_dir(dir.path().join("ortsom.regtest.local.toml")).unwrap();
    let runner = FakeRunner::default();
    runner.script("env", &public_status(dir.path()));
    let context = StackContext::resolve(dir.path(), &runner).await.unwrap();
    assert_eq!(context.lookup("MORTSOM_MOSTRO_PUBKEY").unwrap().len(), 64);
    assert!(
        context
            .lookup("MORTSOM_ALICE_LND_CERT")
            .unwrap()
            .ends_with("ci/regtest/creds/alice/tls.cert")
    );
    assert!(context.lookup("SOLVER_NSEC").is_none());
    assert_eq!(
        context.lookup("MORTSOM_ALICE_LND_HOST").as_deref(),
        Some("https://127.0.0.1:17001")
    );
    assert!(
        runner
            .calls()
            .last()
            .unwrap()
            .ends_with("stack status --json")
    );
}

#[tokio::test]
async fn unready_unknown_or_invalid_stack_descriptor_refuses_attachment() {
    let dir = tempfile::tempdir().unwrap();
    for descriptor in [
        "not json",
        r#"{"schema_version":2,"ready":true,"daemon_pubkey":"invalid"}"#,
        r#"{"schema_version":1,"ready":false,"daemon_pubkey":null}"#,
        r#"{"schema_version":1,"ready":true,"daemon_pubkey":"invalid"}"#,
    ] {
        let runner = FakeRunner::default();
        runner.script("env", descriptor);
        assert!(StackContext::resolve(dir.path(), &runner).await.is_err());
    }
}

#[tokio::test]
async fn failed_stack_output_redacts_wallet_credentials() {
    let root = tempfile::tempdir().unwrap();
    let runner = FakeRunner::default();
    let credential = "nostr+walletconnect://wallet?relay=wss://relay&secret=private";
    runner.script_failure("env", credential);
    let error = invoke(&runner, root.path(), Path::new("ortsom"), "status")
        .await
        .unwrap_err();
    assert!(!error.to_string().contains(credential));
    assert!(!format!("{error:?}").contains(credential));
}

#[tokio::test]
async fn a_failed_latest_pull_leaves_the_existing_stack_untouched() {
    let root = tempfile::tempdir().unwrap();
    let runner = FakeRunner::default();
    runner.script_failure("docker pull", "registry unavailable");
    assert!(
        invoke(&runner, root.path(), Path::new("ortsom"), "up")
            .await
            .is_err()
    );
    assert_eq!(runner.calls().len(), 4);
    assert_eq!(runner.calls()[3], "docker pull mostrop2p/mostro:latest");
}

#[tokio::test]
async fn up_pins_the_resolved_digest_and_verifies_running_image_and_readiness() {
    let root = tempfile::tempdir().unwrap();
    let runner = FakeRunner::default();
    let image_id = format!("sha256:{}", "a".repeat(64));
    let reference = format!("mostrop2p/mostro@sha256:{}", "b".repeat(64));
    runner.script(
        "docker image inspect",
        &serde_json::json!({
            "id": image_id, "digests": [reference],
        })
        .to_string(),
    );
    runner.script(
        "docker inspect",
        &serde_json::json!({"image":image_id,"running":true,
            "working_dir":root.path().join("ci/regtest"),
            "config_files":root.path().join("ci/regtest/compose.yml")})
        .to_string(),
    );
    let base = format!(
        "env --chdir={} COMPOSE_PROJECT_NAME=ortsom-regtest",
        root.path().display()
    );
    runner.script(&format!("{base} docker compose"), &"c".repeat(64));
    runner.script(
        &format!("{base} ortsom stack status --json"),
        &public_status(root.path()),
    );
    invoke(&runner, root.path(), Path::new("ortsom"), "up")
        .await
        .unwrap();
    let calls = runner.calls();
    assert!(calls[5].ends_with(&format!("stack up --mostro-image {reference}")));
    assert!(calls.last().unwrap().ends_with("stack status --json"));
}

#[tokio::test]
async fn foreign_project_blocks_up_and_down_before_any_mutation() {
    let root = tempfile::tempdir().unwrap();
    for action in ["up", "down"] {
        let runner = FakeRunner::default();
        runner.script("docker ps", &"a".repeat(64));
        runner.script(
            "docker inspect",
            r#"{"working_dir":"/foreign","config_files":"/foreign/compose.yml"}"#,
        );
        assert!(
            invoke(&runner, root.path(), Path::new("ortsom"), action)
                .await
                .is_err()
        );
        assert_eq!(runner.calls().len(), 2);
        assert!(
            runner
                .calls()
                .iter()
                .all(|c| !c.starts_with("env") && !c.contains("pull"))
        );
    }
}

#[tokio::test]
async fn attachment_refuses_a_different_checkouts_project() {
    let root = tempfile::tempdir().unwrap();
    let runner = FakeRunner::default();
    runner.script("docker ps", &"a".repeat(64));
    runner.script(
        "docker inspect",
        r#"{"working_dir":"/foreign","config_files":"/foreign/compose.yml"}"#,
    );
    runner.script("env", r#"{"schema_version":1,"ready":true,"daemon_pubkey":"8171eb680049f62ce8747d203f57c3f71de6eea308af6f995363896a1e47cc51"}"#);
    assert!(StackContext::resolve(root.path(), &runner).await.is_err());
}

#[tokio::test]
async fn leftover_resources_block_up_and_down_before_pull_or_lifecycle() {
    let root = tempfile::tempdir().unwrap();
    for action in ["up", "down"] {
        for resource in ["volume", "network"] {
            let runner = FakeRunner::default();
            runner.script(&format!("docker {resource} ls"), "unattributed-resource");
            assert!(
                invoke(&runner, root.path(), Path::new("ortsom"), action)
                    .await
                    .is_err()
            );
            assert!(
                runner
                    .calls()
                    .iter()
                    .all(|c| !c.starts_with("env") && !c.contains("pull"))
            );
        }
    }
}

fn public_status(root: &Path) -> String {
    let wallets: BTreeMap<_, _> = [("alice", 17001), ("bob", 17002)].into_iter().map(|(actor, port)| {
        (actor, serde_json::json!({
            "grpc_url":format!("https://127.0.0.1:{port}"),
            "cert_file":root.join("ci/regtest/creds").join(actor).join("tls.cert"),
            "macaroon_file":root.join("ci/regtest/creds").join(actor).join("admin.macaroon"),
        }))
    }).collect();
    serde_json::json!({"schema_version":1,"ready":true,
        "daemon_pubkey":"8171eb680049f62ce8747d203f57c3f71de6eea308af6f995363896a1e47cc51",
        "connections":{"relay_url":"ws://127.0.0.1:17000", "wallets":wallets},
        "daemon_image":{"image_id":format!("sha256:{}", "a".repeat(64)),
            "reference":"mostrop2p/mostro:latest", "repository_digest":null},
        "declared_limits":{"order_expiration_secs":300,"hold_invoice_expiry_secs":120,
            "buyer_invoice_deadline_secs":120,"payout_invoice_min_expiry_secs":300,
            "fiat_window_secs":null,"max_inflight_payouts_per_destination":2,"payout_dispatch_permits":8}
    })
    .to_string()
}

#[tokio::test]
async fn public_connections_cannot_redirect_credentials_or_share_actor_ports() {
    let root = tempfile::tempdir().unwrap();
    for pointer in [
        "/connections/relay_url",
        "/connections/wallets/alice/grpc_url",
        "/connections/wallets/alice/cert_file",
        "/connections/wallets/bob/macaroon_file",
    ] {
        let mut value: serde_json::Value =
            serde_json::from_str(&public_status(root.path())).unwrap();
        *value.pointer_mut(pointer).unwrap() = serde_json::json!("/outside/credential");
        let runner = FakeRunner::default();
        runner.script("env", &value.to_string());
        assert!(StackContext::resolve(root.path(), &runner).await.is_err());
    }
    let mut value: serde_json::Value = serde_json::from_str(&public_status(root.path())).unwrap();
    value["connections"]["wallets"]["bob"]["grpc_url"] =
        serde_json::json!("https://127.0.0.1:17001");
    let runner = FakeRunner::default();
    runner.script("env", &value.to_string());
    assert!(StackContext::resolve(root.path(), &runner).await.is_err());
}

#[tokio::test]
async fn attachment_requires_valid_image_identity_and_explicit_nonzero_limits() {
    let root = tempfile::tempdir().unwrap();
    for pointer in [
        "/daemon_image",
        "/declared_limits",
        "/declared_limits/order_expiration_secs",
        "/declared_limits/hold_invoice_expiry_secs",
        "/declared_limits/buyer_invoice_deadline_secs",
        "/declared_limits/payout_invoice_min_expiry_secs",
        "/declared_limits/fiat_window_secs",
        "/declared_limits/max_inflight_payouts_per_destination",
        "/declared_limits/payout_dispatch_permits",
    ] {
        let mut status: serde_json::Value =
            serde_json::from_str(&public_status(root.path())).unwrap();
        *status.pointer_mut(pointer).unwrap() = serde_json::json!(0);
        let runner = FakeRunner::default();
        runner.script("env", &status.to_string());
        assert!(
            StackContext::resolve(root.path(), &runner).await.is_err(),
            "{pointer}"
        );
    }
}

#[tokio::test]
async fn literal_daemon_and_wallet_settings_must_match_the_inspected_stack() {
    let root = tempfile::tempdir().unwrap();
    for actor in ["alice", "bob"] {
        let directory = root.path().join("ci/regtest/creds").join(actor);
        std::fs::create_dir_all(&directory).unwrap();
        for name in ["tls.cert", "admin.macaroon"] {
            std::fs::write(directory.join(name), b"fixture").unwrap();
        }
    }
    let runner = FakeRunner::default();
    runner.script("env", &public_status(root.path()));
    let stack = StackContext::resolve(root.path(), &runner).await.unwrap();
    let mut config = crate::Config::from_toml_str(
        include_str!("../../../../config/desktop.example.toml"),
        |name| stack.lookup(name),
    )
    .unwrap();
    stack.configure_relay(&mut config);
    let snapshot = stack.snapshot();
    assert!(snapshot.validate_config(&config).is_ok());
    let original_port = config.relay.host_port;
    config.relay.host_port = 19999;
    assert!(snapshot.validate_config(&config).is_err());
    config.relay.host_port = original_port;
    let original_key = config.mobile.mostro_pubkey.clone();
    config.mobile.mostro_pubkey = nostr_sdk::prelude::Keys::generate().public_key().to_hex();
    assert!(snapshot.validate_config(&config).is_err());
    config.mobile.mostro_pubkey = original_key;
    for actor in ["alice", "bob"] {
        let wallet = if actor == "alice" {
            &mut config.wallets.alice
        } else {
            &mut config.wallets.bob
        };
        let mortsom_environment::WalletConfig::Lnd { host, .. } = wallet else {
            panic!("LND expected");
        };
        let original = host.clone();
        *host = "https://127.0.0.1:9999".into();
        assert!(snapshot.validate_config(&config).is_err());
        let wallet = if actor == "alice" {
            &mut config.wallets.alice
        } else {
            &mut config.wallets.bob
        };
        let mortsom_environment::WalletConfig::Lnd { host, .. } = wallet else {
            panic!("LND expected");
        };
        *host = original;
    }
}
