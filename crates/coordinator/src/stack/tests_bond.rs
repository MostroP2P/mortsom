//! `stack up --bond`: the fragment appended to the daemon's settings, the
//! policy declared to runs, and the daemon's own word on it.
#![allow(clippy::unwrap_used)]

use super::*;

/// A daemon whose info event carries `tags`.
pub(super) struct Advertises(pub(super) &'static [(&'static str, &'static str)]);

/// What mostrod advertises for the committed bond fragment.
const ADVERTISES_THE_FRAGMENT: &[(&str, &str)] = &[
    ("bond_enabled", "true"),
    ("bond_amount_pct", "0.01"),
    ("bond_base_amount_sats", "1000"),
    ("bond_apply_to", "both"),
    ("bond_slash_on_waiting_timeout", "true"),
    ("bond_slash_node_share_pct", "0.5"),
    ("bond_payout_claim_window_days", "1"),
];

#[async_trait]
impl InfoProbe for Advertises {
    async fn daemon_info(
        &self,
        _relay_url: &str,
        pubkey: PublicKey,
    ) -> Option<mortsom_environment::daemon::DaemonInfo> {
        Some(mortsom_environment::daemon::DaemonInfo {
            pubkey: pubkey.to_hex(),
            created_at: 0,
            tags: self
                .0
                .iter()
                .map(|(name, value)| ((*name).to_owned(), (*value).to_owned()))
                .collect(),
        })
    }
}

#[tokio::test]
async fn up_with_bond_appends_the_fragment_to_the_settings_and_declares_the_policy() {
    // Arrange
    let (_dir, layout) = scratch();
    let docker = ScriptedStack::fresh(&layout);
    let options = UpOptions {
        bond: true,
        ..quick()
    };

    // Act
    let descriptor = up(
        &docker,
        &Advertises(ADVERTISES_THE_FRAGMENT),
        &layout,
        &options,
    )
    .await
    .unwrap();

    // Assert
    let settings: toml::Table = std::fs::read_to_string(layout.settings_file())
        .unwrap()
        .parse()
        .unwrap();
    assert_eq!(settings["anti_abuse_bond"]["enabled"].as_bool(), Some(true));
    assert_eq!(
        settings["anti_abuse_bond"]["apply_to"].as_str(),
        Some("both")
    );
    assert!(
        settings["nostr"]["nsec_privkey"]
            .as_str()
            .unwrap()
            .starts_with("nsec1")
    );
    let declared = descriptor.declared_limits.bond.clone().unwrap();
    assert_eq!(declared.apply_to, "both");
    assert_eq!(declared.amount_bps, 100);
    assert_eq!(declared.base_amount_sats, 1000);
    assert!(declared.slash_on_waiting_timeout);
    assert_eq!(declared.node_share_bps, 5000);
    // The policy survives the round trip a run reads it through.
    let reread = Descriptor::read(&layout).unwrap().unwrap();
    assert_eq!(reread.declared_limits.bond, Some(declared));
    assert_eq!(
        reread
            .declared_limits
            .payout_limits()
            .bond
            .map(|policy| policy.bond_sats(1000)),
        Some(1000)
    );
}

/// The settings are what the daemon was told; its info event is what it
/// does. A bonded stack whose daemon advertises no bond (an image older
/// than the bond) is not ready.
#[tokio::test]
async fn up_with_bond_refuses_a_daemon_that_advertises_none() {
    // Arrange
    let (_dir, layout) = scratch();
    let docker = ScriptedStack::fresh(&layout);
    let options = UpOptions {
        bond: true,
        ..quick()
    };

    // Act
    let outcome = up(&docker, &Publishes(true), &layout, &options).await;

    // Assert
    let err = outcome.unwrap_err();
    assert!(err.to_string().contains("advertises none"), "{err}");
}

#[tokio::test]
async fn up_without_bond_leaves_the_daemon_bondless() {
    // Arrange
    let (_dir, layout) = scratch();
    let docker = ScriptedStack::fresh(&layout);

    // Act
    let descriptor = up(&docker, &Publishes(true), &layout, &quick())
        .await
        .unwrap();

    // Assert
    let settings = std::fs::read_to_string(layout.settings_file()).unwrap();
    assert!(!settings.contains("[anti_abuse_bond]"));
    assert_eq!(descriptor.declared_limits.bond, None);
    assert_eq!(descriptor.declared_limits.payout_limits().bond, None);
}

#[tokio::test]
async fn up_with_bond_refuses_a_missing_fragment_before_touching_docker() {
    // Arrange
    let (_dir, layout) = scratch();
    std::fs::remove_file(layout.bond_template()).unwrap();
    let docker = ScriptedStack::fresh(&layout);
    let options = UpOptions {
        bond: true,
        ..quick()
    };

    // Act
    let error = up(&docker, &Publishes(true), &layout, &options)
        .await
        .unwrap_err();

    // Assert
    assert!(
        error.to_string().contains("mostro-bond.tpl.toml"),
        "{error}"
    );
    assert!(docker.matching("pull").is_empty());
}
