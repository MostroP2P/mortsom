//! Relay URL equivalence across vantage points. The same host relay is
//! `127.0.0.1` for the runner and `host.docker.internal` for a containerized
//! daemon; comparisons treat those aliases as one host and otherwise rely
//! on `RelayUrl` normalization.

use nostr_sdk::prelude::RelayUrl;

/// Host names that all mean "the machine running Mortsom".
pub const HOST_ALIASES: &[&str] = &["127.0.0.1", "::1", "localhost", "host.docker.internal"];

/// Canonical comparison key of a relay URL: `(scheme, host-class, port, path)`.
pub fn relay_key(url: &str) -> Option<(String, String, u16, String)> {
    let parsed = RelayUrl::parse(url).ok()?;
    let normalized = parsed.to_string();
    let rest = normalized.split_once("://")?;
    let scheme = rest.0.to_ascii_lowercase();
    let (authority, path) = match rest.1.split_once('/') {
        Some((a, p)) => (a, format!("/{p}")),
        None => (rest.1, "/".to_owned()),
    };
    let (host, port) = match authority.rsplit_once(':') {
        Some((h, p)) if p.chars().all(|c| c.is_ascii_digit()) && !h.is_empty() => {
            (h.to_owned(), p.parse().ok()?)
        }
        _ => (authority.to_owned(), if scheme == "wss" { 443 } else { 80 }),
    };
    let host = host
        .trim_matches(|c| c == '[' || c == ']')
        .to_ascii_lowercase();
    let host_class = if HOST_ALIASES.contains(&host.as_str()) {
        "host".to_owned()
    } else {
        host
    };
    Some((
        scheme,
        host_class,
        port,
        path.trim_end_matches('/').to_owned(),
    ))
}

/// True when both URLs denote the same relay endpoint, treating host aliases
/// as equal.
pub fn same_relay(a: &str, b: &str) -> bool {
    match (relay_key(a), relay_key(b)) {
        (Some(x), Some(y)) => x == y,
        _ => false,
    }
}

/// True when any URL in `candidates` denotes the same relay as any in `wanted`.
pub fn any_same_relay(candidates: &[String], wanted: &[String]) -> bool {
    candidates
        .iter()
        .any(|c| wanted.iter().any(|w| same_relay(c, w)))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn host_aliases_and_trailing_slash_are_equivalent() {
        assert!(same_relay("ws://localhost:7000", "ws://127.0.0.1:7000/"));
        // A daemon on this host may advertise the IPv6 loopback.
        assert!(same_relay("ws://[::1]:7000", "ws://127.0.0.1:7000"));
        assert!(same_relay(
            "ws://host.docker.internal:7000",
            "ws://127.0.0.1:7000"
        ));
        assert!(same_relay("WS://LOCALHOST:7000", "ws://localhost:7000"));
    }

    #[test]
    fn different_ports_hosts_or_schemes_differ() {
        assert!(!same_relay("ws://localhost:7000", "ws://localhost:7001"));
        assert!(!same_relay("ws://localhost:7000", "wss://localhost:7000"));
        assert!(!same_relay("wss://relay.a", "wss://relay.b"));
        assert!(!same_relay("not a url", "ws://localhost:7000"));
    }

    #[test]
    fn default_ports_and_paths_are_considered() {
        assert!(same_relay(
            "wss://relay.example",
            "wss://relay.example:443/"
        ));
        assert!(!same_relay(
            "wss://relay.example/a",
            "wss://relay.example/b"
        ));
        assert!(any_same_relay(
            &["wss://x".into(), "ws://localhost:7000".into()],
            &["ws://host.docker.internal:7000/".into()]
        ));
        assert!(!any_same_relay(&["wss://x".into()], &[]));
    }
}
