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

/// A timeout slash puts the order back in `pending` with the expiry it had
/// at creation, and the claim that follows is paid before the maker cancels
/// it (467 s in `bond_claim_fails_then_replaced`): a bond stack keeps its
/// untaken orders alive longer than a plain one, whose longest-lived order
/// needs about 159 s (#97).
#[tokio::test]
async fn a_bond_stack_keeps_untaken_orders_alive_longer_than_a_plain_one() {
    // Arrange
    let (_plain_dir, plain_layout) = scratch();
    let (_bond_dir, bond_layout) = scratch();
    let bonded = UpOptions {
        bond: true,
        ..quick()
    };

    // Act
    let plain = up(
        &ScriptedStack::fresh(&plain_layout),
        &Publishes(true),
        &plain_layout,
        &quick(),
    )
    .await
    .unwrap();
    let bond = up(
        &ScriptedStack::fresh(&bond_layout),
        &Advertises(ADVERTISES_THE_FRAGMENT),
        &bond_layout,
        &bonded,
    )
    .await
    .unwrap();

    // Assert
    assert_eq!(plain.declared_limits.order_expiration_secs, 360);
    assert_eq!(bond.declared_limits.order_expiration_secs, 600);
}

/// `maker_bond_expires_unpaid` watches the maker's bond lapse, not the
/// order: the bond's timeout, plus the minute the daemon's reaper may take
/// to notice, ends before a bond stack's order would expire.
#[test]
fn a_makers_unpaid_bond_lapses_before_the_order_expires() {
    // Arrange
    const REAPER_TICK_SECS: i64 = 60;
    let (_dir, layout) = scratch();
    let fragment: toml::Table = include_str!("../../../../stack/mostro-bond.tpl.toml")
        .parse()
        .unwrap();

    // Act
    let limits = descriptor::load_limits(&layout.limits_file(), true).unwrap();
    let timeout = fragment["anti_abuse_bond"]["maker_bond_payment_timeout_seconds"]
        .as_integer()
        .unwrap();

    // Assert
    assert!(
        timeout + REAPER_TICK_SECS < i64::try_from(limits.order_expiration_secs).unwrap(),
        "{timeout} s + reaper vs {} s",
        limits.order_expiration_secs
    );
}

/// The daemon's clocks are as short as the catalogue allows (#97): each one
/// bounds a wait some scenario sits through, and is still well above the
/// slowest step measured inside it.
#[test]
fn the_daemon_clocks_are_as_short_as_the_catalogue_allows() {
    // Arrange
    let settings: toml::Table = include_str!("../../../../stack/mostro-settings.tpl.toml")
        .parse()
        .unwrap();
    let fragment: toml::Table = include_str!("../../../../stack/mostro-bond.tpl.toml")
        .parse()
        .unwrap();

    // Assert
    // Restarted at every trade step; the slowest one measured takes 33 s.
    assert_eq!(
        settings["mostro"]["expiration_seconds"].as_integer(),
        Some(90)
    );
    // The maker pays the bond about 22 s after creating the order.
    assert_eq!(
        fragment["anti_abuse_bond"]["maker_bond_payment_timeout_seconds"].as_integer(),
        Some(120)
    );
}

/// Checkouts share one stack directory (`--stack-dir`), and one from before
/// the bond stack had its own lifetime has a limits file without it: a
/// plain stack still reads it, and a bond stack refuses it by name rather
/// than give its orders the plain lifetime.
#[test]
fn a_limits_file_without_the_bond_lifetime_serves_only_a_plain_stack() {
    // Arrange
    let (_dir, layout) = scratch();
    let committed = include_str!("../../../../stack/limits.toml");
    let older: String = committed
        .lines()
        .filter(|line| !line.starts_with("bond_order_expiration_secs"))
        .map(|line| format!("{line}\n"))
        .collect();
    std::fs::write(layout.limits_file(), older).unwrap();

    // Act
    let plain = descriptor::load_limits(&layout.limits_file(), false);
    let bond = descriptor::load_limits(&layout.limits_file(), true);

    // Assert
    assert_eq!(plain.unwrap().order_expiration_secs, 360);
    let refused = bond.unwrap_err().to_string();
    assert!(refused.contains("bond_order_expiration_secs"), "{refused}");
}
