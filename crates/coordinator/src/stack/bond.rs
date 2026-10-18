//! The daemon's anti-abuse bond, switched on per stack (`stack up --bond`):
//! the `[anti_abuse_bond]` fragment appended to the rendered settings, and
//! the policy it declares to every run on that stack. The fragment is the
//! one source: what the daemon reads is what the run is told.

use super::StackError;
use super::metadata::DeclaredBond;

/// The settings table the fragment must carry.
pub const SECTION: &str = "anti_abuse_bond";

/// Reads the `[anti_abuse_bond]` fragment into the policy the stack
/// declares. The daemon must be told to enable it, the side must be one
/// the daemon knows, the percentage a fraction expressible in basis
/// points, and the floor positive: a fragment the daemon would refuse or
/// silently ignore is an error here, before the daemon starts.
pub fn parse_fragment(raw: &str) -> Result<DeclaredBond, StackError> {
    let table: toml::Table = raw
        .parse()
        .map_err(|error| configuration(format!("bond fragment is not TOML: {error}")))?;
    let section = table
        .get(SECTION)
        .and_then(toml::Value::as_table)
        .ok_or_else(|| configuration(format!("bond fragment has no [{SECTION}] table")))?;
    if section.get("enabled").and_then(toml::Value::as_bool) != Some(true) {
        return Err(configuration(format!(
            "[{SECTION}] must set enabled = true; the fragment exists to switch the bond on"
        )));
    }
    let apply_to = section
        .get("apply_to")
        .and_then(toml::Value::as_str)
        .ok_or_else(|| configuration(format!("[{SECTION}].apply_to is missing")))?
        .to_owned();
    let amount_pct = section
        .get("amount_pct")
        .and_then(toml::Value::as_float)
        .ok_or_else(|| configuration(format!("[{SECTION}].amount_pct must be a fraction")))?;
    let amount_bps = basis_points(amount_pct).ok_or_else(|| {
        configuration(format!(
            "[{SECTION}].amount_pct must be a whole number of basis points in 0..=1"
        ))
    })?;
    let base_amount_sats = section
        .get("base_amount_sats")
        .and_then(toml::Value::as_integer)
        .and_then(|sats| u64::try_from(sats).ok())
        .ok_or_else(|| {
            configuration(format!("[{SECTION}].base_amount_sats must be a sat amount"))
        })?;
    let slash_on_waiting_timeout = section
        .get("slash_on_waiting_timeout")
        .and_then(toml::Value::as_bool)
        .unwrap_or(false);
    let node_share_bps = section
        .get("slash_node_share_pct")
        .and_then(toml::Value::as_float)
        .and_then(basis_points)
        .ok_or_else(|| {
            configuration(format!(
                "[{SECTION}].slash_node_share_pct must be a whole number of basis points in 0..=1"
            ))
        })?;
    let payout_claim_window_days = section
        .get("payout_claim_window_days")
        .and_then(toml::Value::as_integer)
        .and_then(|days| u32::try_from(days).ok())
        .ok_or_else(|| {
            configuration(format!(
                "[{SECTION}].payout_claim_window_days must be a number of days"
            ))
        })?;
    let bond = DeclaredBond {
        apply_to,
        amount_bps,
        base_amount_sats,
        slash_on_waiting_timeout,
        node_share_bps,
        payout_claim_window_days,
    };
    bond.validate()?;
    Ok(bond)
}

/// A fraction in `0..=1` as basis points, when it is a whole number of them.
fn basis_points(fraction: f64) -> Option<u32> {
    if !fraction.is_finite() || !(0.0..=1.0).contains(&fraction) {
        return None;
    }
    let scaled = fraction * f64::from(mortsom_scenario_kit::context::BPS_PER_WHOLE);
    let rounded = scaled.round();
    if (scaled - rounded).abs() > 1e-6 {
        return None;
    }
    // `rounded` is within 0..=10_000 here.
    Some(rounded as u32)
}

/// The rendered settings with the fragment appended. The base template
/// must not carry the table itself: two `[anti_abuse_bond]` tables would
/// make the settings invalid TOML, and a template that enables the bond
/// on its own would bond every family.
pub fn append_fragment(settings: &str, fragment: &str) -> Result<String, StackError> {
    if settings.contains(&format!("[{SECTION}]")) {
        return Err(configuration(format!(
            "the settings template already carries [{SECTION}]; the bond is switched on with `stack up --bond` only"
        )));
    }
    let mut rendered = settings.to_owned();
    if !rendered.ends_with('\n') {
        rendered.push('\n');
    }
    rendered.push('\n');
    rendered.push_str(fragment);
    if !rendered.ends_with('\n') {
        rendered.push('\n');
    }
    Ok(rendered)
}

/// Checks that the daemon runs the bond the stack declares, from the
/// policy its info event advertises (kind 38385, `bond_*` tags): a
/// declared bond must be advertised exactly, and a stack that declares
/// none must run a daemon that asks for none. The settings file is only
/// what the daemon was told; this is what it says it does.
pub fn verify_advertised(
    info: &mortsom_environment::daemon::DaemonInfo,
    declared: Option<&DeclaredBond>,
) -> Result<(), StackError> {
    let enabled = info.tag("bond_enabled").map(str::trim) == Some("true");
    let Some(declared) = declared else {
        return if enabled {
            Err(configuration(
                "the daemon advertises an anti-abuse bond the stack does not declare; \
                 the bond is switched on with `stack up --bond` only"
                    .into(),
            ))
        } else {
            Ok(())
        };
    };
    if !enabled {
        return Err(configuration(
            "the stack declares an anti-abuse bond and the daemon advertises none \
             (`bond_enabled`); is the image older than the bond?"
                .into(),
        ));
    }
    let fraction = |tag: &str| {
        info.tag(tag)
            .and_then(|value| value.trim().parse::<f64>().ok())
            .and_then(basis_points)
    };
    let advertised = DeclaredBond {
        apply_to: info.tag("bond_apply_to").unwrap_or_default().trim().into(),
        amount_bps: fraction("bond_amount_pct").unwrap_or(u32::MAX),
        base_amount_sats: info.tag_u64("bond_base_amount_sats").unwrap_or_default(),
        slash_on_waiting_timeout: info.tag("bond_slash_on_waiting_timeout").map(str::trim)
            == Some("true"),
        node_share_bps: fraction("bond_slash_node_share_pct").unwrap_or(u32::MAX),
        payout_claim_window_days: info
            .tag_u64("bond_payout_claim_window_days")
            .and_then(|days| u32::try_from(days).ok())
            .unwrap_or_default(),
    };
    if &advertised != declared {
        return Err(configuration(format!(
            "the daemon advertises the bond {advertised:?}, and the stack declares {declared:?}"
        )));
    }
    Ok(())
}

/// The bond line of `stack ready`: the side that posts, the share and the
/// floor, or `off` for a bondless daemon.
pub fn describe(bond: Option<&DeclaredBond>) -> String {
    match bond {
        None => "off".to_owned(),
        Some(bond) => format!(
            "{} ({} bps of the order, at least {} sats{})",
            bond.apply_to,
            bond.amount_bps,
            bond.base_amount_sats,
            if bond.slash_on_waiting_timeout {
                ", slashed on a waiting timeout"
            } else {
                ""
            }
        ),
    }
}

fn configuration(message: String) -> StackError {
    StackError::Configuration(message)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    const FRAGMENT: &str = include_str!("../../../../stack/mostro-bond.tpl.toml");

    #[test]
    fn the_committed_fragment_bonds_both_sides_and_declares_the_split() {
        let bond = parse_fragment(FRAGMENT).unwrap();
        assert_eq!(
            bond,
            DeclaredBond {
                apply_to: "both".into(),
                amount_bps: 100,
                base_amount_sats: 1000,
                slash_on_waiting_timeout: true,
                node_share_bps: 5000,
                payout_claim_window_days: 1,
            }
        );
        // The daemon reads the same table the run is told about.
        let table: toml::Table = FRAGMENT.parse().unwrap();
        assert_eq!(table[SECTION]["enabled"].as_bool(), Some(true));
        assert_eq!(table[SECTION]["apply_to"].as_str(), Some("both"));
    }

    #[test]
    fn a_fragment_the_daemon_would_refuse_or_ignore_is_refused_first() {
        for (broken, why) in [
            (
                "[anti_abuse_bond]\nenabled = false\napply_to = \"take\"\namount_pct = 0.01\nbase_amount_sats = 1000\nslash_node_share_pct = 0.5\npayout_claim_window_days = 1\n",
                "disabled",
            ),
            (
                "[anti_abuse_bond]\nenabled = true\napply_to = \"taker\"\namount_pct = 0.01\nbase_amount_sats = 1000\n",
                "unknown side",
            ),
            (
                "[anti_abuse_bond]\nenabled = true\napply_to = \"take\"\namount_pct = 0.00001\nbase_amount_sats = 1000\n",
                "not whole basis points",
            ),
            (
                "[anti_abuse_bond]\nenabled = true\napply_to = \"take\"\namount_pct = 1.5\nbase_amount_sats = 1000\n",
                "over one",
            ),
            (
                "[anti_abuse_bond]\nenabled = true\napply_to = \"take\"\namount_pct = 0.01\nbase_amount_sats = 0\n",
                "no floor",
            ),
            ("[lightning]\n", "no table"),
            ("not toml [", "not toml"),
        ] {
            assert!(parse_fragment(broken).is_err(), "{why}");
        }
    }

    #[test]
    fn appending_keeps_both_tables_readable_as_one_settings_file() {
        let rendered = append_fragment("[mostro]\nfee = 0.01", FRAGMENT).unwrap();
        let table: toml::Table = rendered.parse().unwrap();
        assert_eq!(table["mostro"]["fee"].as_float(), Some(0.01));
        assert_eq!(table[SECTION]["enabled"].as_bool(), Some(true));
        assert!(rendered.ends_with('\n'));
    }

    #[test]
    fn a_template_that_already_bonds_every_family_is_refused() {
        let err = append_fragment("[anti_abuse_bond]\nenabled = true\n", FRAGMENT).unwrap_err();
        assert!(err.to_string().contains("--bond"), "{err}");
    }

    fn info(tags: &[(&str, &str)]) -> mortsom_environment::daemon::DaemonInfo {
        mortsom_environment::daemon::DaemonInfo {
            pubkey: "daemon".into(),
            created_at: 0,
            tags: tags
                .iter()
                .map(|(name, value)| ((*name).to_owned(), (*value).to_owned()))
                .collect(),
        }
    }

    /// The tags mostrod writes for the committed fragment (`nip33.rs`,
    /// `bond_policy_tags`: floats through `to_string`).
    fn advertising_the_fragment() -> mortsom_environment::daemon::DaemonInfo {
        info(&[
            ("bond_enabled", "true"),
            ("bond_amount_pct", "0.01"),
            ("bond_base_amount_sats", "1000"),
            ("bond_apply_to", "both"),
            ("bond_slash_on_waiting_timeout", "true"),
            ("bond_slash_node_share_pct", "0.5"),
            ("bond_payout_claim_window_days", "1"),
        ])
    }

    #[test]
    fn a_daemon_advertising_the_declared_bond_passes() {
        let declared = parse_fragment(FRAGMENT).unwrap();
        verify_advertised(&advertising_the_fragment(), Some(&declared)).unwrap();
    }

    #[test]
    fn a_daemon_advertising_another_bond_is_refused() {
        let declared = parse_fragment(FRAGMENT).unwrap();
        let mut other = advertising_the_fragment();
        other.tags.insert("bond_apply_to".into(), "take".into());
        let err = verify_advertised(&other, Some(&declared)).unwrap_err();
        assert!(err.to_string().contains("advertises the bond"), "{err}");
    }

    /// An image older than the bond, or a fragment the daemon ignored,
    /// advertises none: the stack must not declare one it does not run.
    #[test]
    fn a_declared_bond_the_daemon_does_not_advertise_is_refused() {
        let declared = parse_fragment(FRAGMENT).unwrap();
        for silent in [info(&[]), info(&[("bond_enabled", "false")])] {
            assert!(verify_advertised(&silent, Some(&declared)).is_err());
        }
    }

    #[test]
    fn a_bondless_stack_refuses_a_daemon_that_asks_for_bonds() {
        verify_advertised(&info(&[("bond_enabled", "false")]), None).unwrap();
        verify_advertised(&info(&[]), None).unwrap();
        let err = verify_advertised(&advertising_the_fragment(), None).unwrap_err();
        assert!(err.to_string().contains("--bond"), "{err}");
    }

    #[test]
    fn the_stack_ready_line_names_the_bond_policy_or_says_off() {
        assert_eq!(describe(None), "off");
        let bond = DeclaredBond {
            apply_to: "take".into(),
            amount_bps: 100,
            base_amount_sats: 1000,
            slash_on_waiting_timeout: false,
            node_share_bps: 5000,
            payout_claim_window_days: 1,
        };
        assert_eq!(
            describe(Some(&bond)),
            "take (100 bps of the order, at least 1000 sats)"
        );
        let slashing = DeclaredBond {
            slash_on_waiting_timeout: true,
            ..bond
        };
        assert!(describe(Some(&slashing)).ends_with("slashed on a waiting timeout)"));
    }

    #[test]
    fn basis_points_are_exact_or_nothing() {
        assert_eq!(basis_points(0.01), Some(100));
        assert_eq!(basis_points(0.0), Some(0));
        assert_eq!(basis_points(1.0), Some(10_000));
        assert_eq!(basis_points(0.00015), None);
        assert_eq!(basis_points(-0.01), None);
        assert_eq!(basis_points(f64::NAN), None);
    }
}
