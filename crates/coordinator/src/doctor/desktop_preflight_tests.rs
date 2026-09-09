use super::*;

fn ready_net() -> FakeNet {
    let mut net = FakeNet::healthy();
    net.port = PortState::InUse;
    net.network = Some("regtest".into());
    net.methods = ["pay_invoice", "make_invoice", "lookup_invoice"]
        .map(str::to_owned)
        .to_vec();
    net
}

fn loaded_config() -> (tempfile::TempDir, Config) {
    let root = example_config();
    let host = FakeHost::full();
    let config =
        Config::load_with(&root.path().join("mortsom.toml"), |name| host.env(name)).unwrap();
    (root, config)
}

#[tokio::test]
async fn only_fresh_protocol_two_info_can_supply_a_verified_daemon_version() {
    let (_root, config) = loaded_config();
    let ready = desktop_preflight(&config, &ready_net()).await;
    assert!(ready.checks.is_healthy(), "{}", ready.checks.render());
    assert_eq!(ready.daemon_version.as_deref(), Some("0.18.4"));
    for fault in [
        "missing-info",
        "missing-version",
        "invalid-version",
        "old-protocol",
        "stale-info",
    ] {
        let mut net = ready_net();
        match fault {
            "missing-info" => net.daemon.info = None,
            "missing-version" => {
                net.daemon
                    .info
                    .as_mut()
                    .unwrap()
                    .tags
                    .remove("mostro_version");
            }
            "invalid-version" => {
                net.daemon
                    .info
                    .as_mut()
                    .unwrap()
                    .tags
                    .insert("mostro_version".into(), "invalid version".into());
            }
            "old-protocol" => {
                net.daemon
                    .info
                    .as_mut()
                    .unwrap()
                    .tags
                    .insert("protocol_version".into(), "1".into());
            }
            "stale-info" => net.daemon.info.as_mut().unwrap().created_at = 0,
            _ => unreachable!(),
        }
        let result = desktop_preflight(&config, &net).await;
        assert!(!result.checks.is_healthy(), "{fault}");
        assert!(result.daemon_version.is_none(), "rejected version: {fault}");
    }
}

#[tokio::test]
async fn wallet_network_and_payment_capabilities_are_required_before_clients_start() {
    let (_root, config) = loaded_config();
    for network in [None, Some("mainnet".into())] {
        let mut net = ready_net();
        net.network = network;
        assert!(!desktop_preflight(&config, &net).await.checks.is_healthy());
    }
    for method in ["pay_invoice", "make_invoice", "lookup_invoice"] {
        let mut net = ready_net();
        net.methods.retain(|value| value != method);
        assert!(
            !desktop_preflight(&config, &net).await.checks.is_healthy(),
            "{method}"
        );
    }
}
