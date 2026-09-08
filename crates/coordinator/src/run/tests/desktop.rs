use super::*;

fn desktop_options(world: &World) -> RunOptions {
    let mut options = opts(&["happy_sell"]);
    let source = world.dir.path().join("bundle");
    std::fs::create_dir_all(&source).unwrap();
    std::fs::write(source.join("mostro"), b"fixture").unwrap();
    options.desktop = Some(crate::run::DesktopOptions {
        platform: mortsom_desktop_driver::Platform::Linux,
        artifact: world.dir.path().join("missing-app"),
        xvfb: "missing-Xvfb".into(),
        browser: "missing-browser".into(),
        chromedriver: "missing-driver".into(),
        bundle: Arc::new(crate::fingerprint::CapturedBundle::capture(&source).unwrap()),
    });
    options
}

#[tokio::test]
async fn missing_stack_metadata_fails_before_creating_clients() {
    mortsom_scenarios::link();
    let world = world(true);
    let options = desktop_options(&world);
    let summary = run(&world, Arc::new(FakeAdapter::full()), options, true)
        .await
        .unwrap();
    assert!(!summary.passed);
    assert!(summary.actors.is_empty() && summary.farm_teardown.is_empty());
    assert!(summary.relay.is_none());
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
    let mut options = desktop_options(&world);
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
