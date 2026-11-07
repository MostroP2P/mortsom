//! `--profile` in place of the configured `mobile.profile` (spec §7).
use mortsom_app_contract::config::KNOWN_PROFILES;
use toml::Value;

/// Writes `flag` (`--profile`) over `mobile.profile` in the raw layered
/// configuration, before its environment references are resolved and it is
/// validated. The configured value is then never read, so the flag also
/// stands in for a reference to an unset (or empty) variable or for an
/// unknown configured profile, and the loaded configuration names the
/// profile the invocation drives.
///
/// Only a known profile is written. An unknown flag replaces nothing: the
/// configuration loads or fails as without it, and resolving the client
/// (`mostro_cli::resolve_client`, doctor's `client` check) refuses the
/// flag by name, never as a configuration problem about `mobile.profile`.
pub(super) fn replace_profile(raw: &mut Value, flag: Option<&str>) {
    let Some(flag) = flag.filter(|flag| KNOWN_PROFILES.contains(flag)) else {
        return;
    };
    if let Some(mobile) = raw.get_mut("mobile").and_then(Value::as_table_mut) {
        mobile.insert("profile".to_owned(), Value::String(flag.to_owned()));
    }
}
