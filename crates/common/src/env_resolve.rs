//! Resolution of `env:NAME` references and `${NAME}` interpolations inside a
//! configuration tree.
//!
//! Resolution fails closed: every missing variable is collected and reported
//! at once, and nothing downstream (containers, clients, wallets) can start
//! until the tree is fully resolved.

use std::collections::BTreeSet;

use thiserror::Error;
use toml::Value;

/// Prefix marking a whole-string environment reference.
pub const ENV_PREFIX: &str = "env:";

/// One or more referenced environment variables were not set.
#[derive(Debug, Error, PartialEq, Eq)]
#[error("missing environment variables: {}", .0.join(", "))]
pub struct MissingEnv(pub Vec<String>);

/// Resolves every `env:` reference and `${NAME}` interpolation in `value`
/// using `lookup`. All missing names are reported together, sorted and
/// deduplicated.
pub fn resolve_env_with<F>(value: Value, lookup: F) -> Result<Value, MissingEnv>
where
    F: Fn(&str) -> Option<String>,
{
    let mut missing = BTreeSet::new();
    let resolved = walk(value, &lookup, &mut missing);
    if missing.is_empty() {
        Ok(resolved)
    } else {
        Err(MissingEnv(missing.into_iter().collect()))
    }
}

/// Like [`resolve_env_with`] but substitutes an empty string for every
/// missing variable and returns the names alongside the tree. Intended only
/// for diagnostics such as `mortsom doctor`, which must keep going to report
/// every problem instead of stopping at the first one.
pub fn resolve_env_lenient<F>(value: Value, lookup: F) -> (Value, Vec<String>)
where
    F: Fn(&str) -> Option<String>,
{
    let mut missing = BTreeSet::new();
    let resolved = walk(value, &lookup, &mut missing);
    (resolved, missing.into_iter().collect())
}

/// Returns the environment variable names referenced anywhere in `value`.
pub fn referenced_env_names(value: &Value) -> Vec<String> {
    let mut names = BTreeSet::new();
    collect_names(value, &mut names);
    names.into_iter().collect()
}

fn walk<F>(value: Value, lookup: &F, missing: &mut BTreeSet<String>) -> Value
where
    F: Fn(&str) -> Option<String>,
{
    match value {
        Value::String(text) => Value::String(resolve_string(&text, lookup, missing)),
        Value::Array(items) => Value::Array(
            items
                .into_iter()
                .map(|item| walk(item, lookup, missing))
                .collect(),
        ),
        Value::Table(table) => Value::Table(
            table
                .into_iter()
                .map(|(key, item)| (key, walk(item, lookup, missing)))
                .collect(),
        ),
        other => other,
    }
}

fn resolve_string<F>(text: &str, lookup: &F, missing: &mut BTreeSet<String>) -> String
where
    F: Fn(&str) -> Option<String>,
{
    if let Some(name) = text.strip_prefix(ENV_PREFIX) {
        return match lookup(name) {
            Some(value) => value,
            None => {
                missing.insert(name.to_owned());
                String::new()
            }
        };
    }
    interpolate(text, lookup, missing)
}

fn interpolate<F>(text: &str, lookup: &F, missing: &mut BTreeSet<String>) -> String
where
    F: Fn(&str) -> Option<String>,
{
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find("${") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        match after.find('}') {
            Some(end) => {
                let name = &after[..end];
                match lookup(name) {
                    Some(value) => out.push_str(&value),
                    None => {
                        missing.insert(name.to_owned());
                    }
                }
                rest = &after[end + 1..];
            }
            None => {
                out.push_str(&rest[start..]);
                rest = "";
            }
        }
    }
    out.push_str(rest);
    out
}

fn collect_names(value: &Value, names: &mut BTreeSet<String>) {
    match value {
        Value::String(text) => {
            let mut sink = BTreeSet::new();
            let _ = resolve_string(text, &|_| None, &mut sink);
            names.extend(sink);
        }
        Value::Array(items) => items.iter().for_each(|item| collect_names(item, names)),
        Value::Table(table) => table.values().for_each(|item| collect_names(item, names)),
        _ => {}
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    fn lookup(name: &str) -> Option<String> {
        match name {
            "PUBKEY" => Some("abc123".into()),
            "ALICE" => Some("nostr+walletconnect://alice".into()),
            _ => None,
        }
    }

    fn parse(text: &str) -> Value {
        toml::from_str(text).unwrap()
    }

    #[test]
    fn whole_string_reference_is_replaced() {
        let value = parse(r#"pubkey = "env:PUBKEY""#);

        let resolved = resolve_env_with(value, lookup).unwrap();

        assert_eq!(resolved["pubkey"].as_str(), Some("abc123"));
    }

    #[test]
    fn interpolation_inside_arrays_and_nested_tables_is_replaced() {
        let value = parse(
            r#"
            [mobile]
            dart_defines = ["MOSTRO_PUB_KEY=${PUBKEY}", "plain"]
            [wallets]
            alice_nwc = "env:ALICE"
            "#,
        );

        let resolved = resolve_env_with(value, lookup).unwrap();

        assert_eq!(
            resolved["mobile"]["dart_defines"][0].as_str(),
            Some("MOSTRO_PUB_KEY=abc123")
        );
        assert_eq!(
            resolved["mobile"]["dart_defines"][1].as_str(),
            Some("plain")
        );
        assert_eq!(
            resolved["wallets"]["alice_nwc"].as_str(),
            Some("nostr+walletconnect://alice")
        );
    }

    #[test]
    fn every_missing_variable_is_reported_once_sorted() {
        let value = parse(
            r#"
            a = "env:ZED"
            b = "prefix ${ALPHA} and ${ZED}"
            [t]
            c = ["env:MID", "env:ALPHA"]
            "#,
        );

        let err = resolve_env_with(value, lookup).unwrap_err();

        assert_eq!(
            err,
            MissingEnv(vec!["ALPHA".into(), "MID".into(), "ZED".into()])
        );
        assert_eq!(
            err.to_string(),
            "missing environment variables: ALPHA, MID, ZED"
        );
    }

    #[test]
    fn non_string_values_and_plain_strings_pass_through() {
        let value = parse(
            r#"
            n = 7
            b = true
            s = "no references here"
            "#,
        );

        let resolved = resolve_env_with(value.clone(), lookup).unwrap();

        assert_eq!(resolved, value);
    }

    #[test]
    fn unterminated_interpolation_is_left_verbatim() {
        let value = parse(r#"s = "broken ${PUBKEY""#);

        let resolved = resolve_env_with(value, lookup).unwrap();

        assert_eq!(resolved["s"].as_str(), Some("broken ${PUBKEY"));
    }

    #[test]
    fn lenient_resolution_substitutes_empty_and_lists_missing() {
        let value = parse(
            r#"a = "env:NOPE"
b = "env:PUBKEY""#,
        );

        let (resolved, missing) = resolve_env_lenient(value, lookup);

        assert_eq!(resolved["a"].as_str(), Some(""));
        assert_eq!(resolved["b"].as_str(), Some("abc123"));
        assert_eq!(missing, vec!["NOPE".to_owned()]);
    }

    #[test]
    fn referenced_names_are_listed_without_resolving() {
        let value = parse(
            r#"
            a = "env:ONE"
            b = "x ${TWO} y ${ONE}"
            "#,
        );

        assert_eq!(
            referenced_env_names(&value),
            vec!["ONE".to_owned(), "TWO".to_owned()]
        );
    }
}
