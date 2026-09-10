use super::*;

fn no_stack(world: &World) -> RunOptions {
    let mut options = opts_in(world, &["happy_sell"]);
    options.stack = None;
    options
}

#[tokio::test]
async fn missing_stack_metadata_fails_before_creating_clients() {
    mortsom_scenarios::link();
    let world = world(true);
    let options = no_stack(&world);
    let summary = run(&world, Arc::new(FakeAdapter::full()), options, true)
        .await
        .unwrap();
    assert!(!summary.passed);
    assert!(summary.actors.is_empty() && summary.farm_teardown.is_empty());
    assert!(summary.results.iter().all(|result| result.attempts == 0));
    assert!(
        summary
            .checks
            .checks
            .iter()
            .any(|check| check.name == "desktop-preflight" && check.status == CheckStatus::Fail)
    );
}

#[tokio::test]
async fn a_different_stack_identity_is_rejected_before_network_or_client_startup() {
    mortsom_scenarios::link();
    let world = world(true);
    let mut options = opts_in(&world, &["happy_sell"]);
    options.stack = Some(
        serde_json::from_value(serde_json::json!({
            "daemon_pubkey": nostr_sdk::prelude::Keys::generate().public_key().to_hex(),
            "relay_url":"ws://127.0.0.1:7000","wallet_endpoints":{},
            "daemon_image":{"image_id":format!("sha256:{}", "a".repeat(64)),
                "reference":"mostrop2p/mostro:latest","repository_digest":null},
            "declared_limits":{"order_expiration_secs":300,"hold_invoice_expiry_secs":120,
                "buyer_invoice_deadline_secs":120,"payout_invoice_min_expiry_secs":300}
        }))
        .unwrap(),
    );
    let summary = run(&world, Arc::new(FakeAdapter::full()), options, true)
        .await
        .unwrap();
    assert!(!summary.passed);
    assert!(summary.actors.is_empty() && summary.farm_teardown.is_empty());
    assert!(
        summary
            .checks
            .checks
            .iter()
            .any(|check| check.detail.contains("binding"))
    );
    assert!(
        !summary
            .checks
            .checks
            .iter()
            .any(|check| check.detail.contains("network probe is required"))
    );
}

#[test]
fn initial_environment_retains_public_bundle_and_stack_facts_before_clients_start() {
    let world = world(true);
    let mut options = opts_in(&world, &["happy_sell"]);
    options.stack = Some(
        serde_json::from_value(serde_json::json!({
            "daemon_pubkey": nostr_sdk::prelude::Keys::generate().public_key().to_hex(),
            "relay_url":"ws://127.0.0.1:7000","wallet_endpoints":{},
            "daemon_image":{"image_id":format!("sha256:{}", "a".repeat(64)),
                "reference":"mostrop2p/mostro:latest","repository_digest":null},
            "declared_limits":{"order_expiration_secs":300,"hold_invoice_expiry_secs":120,
                "buyer_invoice_deadline_secs":120,"payout_invoice_min_expiry_secs":300}
        }))
        .unwrap(),
    );
    let mut summary = crate::run::report::initial_summary(
        &world.config,
        &options,
        RunId::new(),
        "mobile-v2".into(),
        false,
    )
    .unwrap();
    summary.mostrod_version = Some("0.18.7".into());
    let artifacts = RunArtifacts::create(
        world.dir.path(),
        summary.run_id.clone(),
        mortsom_common::Redactor::new(),
    )
    .unwrap();
    crate::run::report::record_environment(&artifacts, &summary).unwrap();
    let saved: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(artifacts.file(files::ENVIRONMENT).unwrap()).unwrap(),
    )
    .unwrap();
    assert_eq!(saved["status"], "starting");
    assert_eq!(saved["mostrod_version"], "0.18.7");
    assert_eq!(saved["app_build"]["origin"], "prebuilt");
    assert_eq!(
        saved["app_artifact"]["digest"],
        summary.app_artifact.as_ref().unwrap().digest
    );
    assert_eq!(
        saved["stack_fingerprint"]["digest"],
        summary.stack_fingerprint.as_ref().unwrap().digest
    );
    assert!(
        !artifacts.file(files::SUMMARY).unwrap().exists(),
        "this evidence precedes the final summary"
    );
}
