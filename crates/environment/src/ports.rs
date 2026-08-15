//! Host port allocation for run-owned services.

use std::net::{Ipv4Addr, SocketAddrV4, TcpListener};

/// How many consecutive ports are tried above the configured base.
pub const PORT_SEARCH_WINDOW: u16 = 100;

/// Returns the first port in `[base, base + PORT_SEARCH_WINDOW)` that can be
/// bound on all interfaces right now, or `None`. Callers should bind their
/// service immediately; the check is inherently best-effort.
pub fn allocate_port(base: u16) -> Option<u16> {
    (0..PORT_SEARCH_WINDOW)
        .filter_map(|offset| base.checked_add(offset))
        .find(|port| TcpListener::bind(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, *port)).is_ok())
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn allocates_the_base_when_free_and_skips_when_busy() {
        // Pick a base the OS considers free right now.
        let probe = TcpListener::bind((Ipv4Addr::UNSPECIFIED, 0)).unwrap();
        let base = probe.local_addr().unwrap().port();
        drop(probe);
        assert_eq!(allocate_port(base), Some(base));

        let holder = TcpListener::bind((Ipv4Addr::UNSPECIFIED, base)).unwrap();
        let next = allocate_port(base).unwrap();
        assert!(next > base && next < base + PORT_SEARCH_WINDOW, "{next}");
        drop(holder);
    }

    #[test]
    fn overflow_near_the_top_of_the_range_returns_none_or_valid() {
        if let Some(port) = allocate_port(u16::MAX) {
            assert_eq!(port, u16::MAX);
        }
    }
}
