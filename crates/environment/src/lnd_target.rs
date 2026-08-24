//! Turning an `lnd` wallet config into something the gRPC client accepts.
//!
//! The values a user copies out of Polar's *Connect* tab are not the ones
//! `fedimint_tonic_lnd::connect` wants: the host comes without a scheme,
//! and the credential paths are written with a `~`, which nothing below
//! the shell expands. Both are normalised here, in one place, so the
//! config file can be filled in exactly as Polar displays it.

use std::path::PathBuf;

/// Scheme assumed when the configured host has none.
///
/// LND's gRPC endpoint is TLS-only and its certificate is what the `cert`
/// field points at, so this is not a real choice — a plain `http://`
/// endpoint would never answer.
const DEFAULT_SCHEME: &str = "https://";

/// The endpoint string for the gRPC client.
///
/// Polar shows `127.0.0.1:10001`; other tools print a full URL. Both are
/// accepted, and anything already carrying a scheme is left alone.
pub fn grpc_endpoint(host: &str) -> String {
    let host = host.trim();
    if host.contains("://") {
        host.to_owned()
    } else {
        format!("{DEFAULT_SCHEME}{host}")
    }
}

/// A credential path with a leading `~` expanded against `$HOME`.
///
/// Polar's own documentation writes its paths that way, and a config file
/// is not a shell: without this, the most natural thing to paste is also
/// the thing that fails.
pub fn credential_path(raw: &str) -> PathBuf {
    credential_path_with(raw, || std::env::var("HOME").ok())
}

/// [`credential_path`] with the home directory injected, for tests.
pub fn credential_path_with<F>(raw: &str, home: F) -> PathBuf
where
    F: FnOnce() -> Option<String>,
{
    let raw = raw.trim();
    // Only a bare `~` or `~/…` is a home reference. `~user/…` is not
    // something this resolves, and neither is a `~` further along the
    // path — both stay as written rather than being silently mangled.
    let rest = match raw {
        "~" => Some(""),
        _ => raw.strip_prefix("~/"),
    };
    match (rest, home()) {
        (Some(rest), Some(home)) => PathBuf::from(home).join(rest),
        _ => PathBuf::from(raw),
    }
}

/// Whether a gRPC host carries a port, with or without a scheme.
///
/// LND's gRPC port is never the default 443, so an endpoint without one
/// connects somewhere else rather than failing honestly.
pub fn host_has_a_port(host: &str) -> bool {
    let authority = host.trim().rsplit("://").next().unwrap_or_default();
    // In an IPv6 authority the colons belong to the address; only a colon
    // after the closing bracket is the port separator.
    let tail = match authority.rsplit_once(']') {
        Some((_, after)) => after,
        None if authority.matches(':').count() > 1 => return false,
        None => authority,
    };
    tail.rsplit_once(':')
        .is_some_and(|(_, port)| !port.is_empty() && port.chars().all(|c| c.is_ascii_digit()))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn adds_the_scheme_polar_omits() {
        assert_eq!(grpc_endpoint("127.0.0.1:10001"), "https://127.0.0.1:10001");
    }

    #[test]
    fn leaves_an_explicit_scheme_alone() {
        assert_eq!(
            grpc_endpoint("https://127.0.0.1:10009"),
            "https://127.0.0.1:10009"
        );
        assert_eq!(
            grpc_endpoint("http://node.local:10009"),
            "http://node.local:10009"
        );
    }

    #[test]
    fn ignores_surrounding_whitespace_from_a_paste() {
        assert_eq!(
            grpc_endpoint("  127.0.0.1:10001 "),
            "https://127.0.0.1:10001"
        );
    }

    #[test]
    fn expands_a_leading_tilde_against_home() {
        let path = credential_path_with("~/.polar/networks/1/tls.cert", || Some("/home/u".into()));

        assert_eq!(path, PathBuf::from("/home/u/.polar/networks/1/tls.cert"));
    }

    #[test]
    fn expands_a_bare_tilde() {
        assert_eq!(
            credential_path_with("~", || Some("/home/u".into())),
            PathBuf::from("/home/u")
        );
    }

    #[test]
    fn leaves_an_absolute_path_untouched() {
        assert_eq!(
            credential_path_with("/etc/lnd/tls.cert", || Some("/home/u".into())),
            PathBuf::from("/etc/lnd/tls.cert")
        );
    }

    /// A `~` that is not a home reference must survive verbatim: mangling
    /// it would produce a path the user never wrote.
    #[test]
    fn leaves_a_non_home_tilde_untouched() {
        assert_eq!(
            credential_path_with("~alice/tls.cert", || Some("/home/u".into())),
            PathBuf::from("~alice/tls.cert")
        );
        assert_eq!(
            credential_path_with("/certs/~backup/tls.cert", || Some("/home/u".into())),
            PathBuf::from("/certs/~backup/tls.cert")
        );
    }

    /// Without `$HOME` there is nothing to expand against; keeping the raw
    /// path makes the eventual failure name the path the user wrote.
    #[test]
    fn keeps_the_raw_path_when_home_is_unset() {
        assert_eq!(
            credential_path_with("~/tls.cert", || None),
            PathBuf::from("~/tls.cert")
        );
    }

    #[test]
    fn host_port_detection_handles_schemes_and_ipv6() {
        assert!(host_has_a_port("127.0.0.1:10009"));
        assert!(host_has_a_port("https://127.0.0.1:10009"));
        assert!(host_has_a_port("[::1]:10009"));
        assert!(!host_has_a_port("127.0.0.1"));
        assert!(!host_has_a_port("https://node.local"));
        assert!(!host_has_a_port("[::1]"));
    }
}
