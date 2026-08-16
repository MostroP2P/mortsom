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
        // Hold a port so it is busy for sure, then allocate from it: the
        // result must be a different, higher port in the window.
        let holder = TcpListener::bind((Ipv4Addr::UNSPECIFIED, 0)).unwrap();
        let base = holder.local_addr().unwrap().port();
        let next = allocate_port(base).unwrap();
        // Widened: a base near the top of the range would overflow u16.
        let ceiling = u32::from(base) + u32::from(PORT_SEARCH_WINDOW);
        assert!(next > base && u32::from(next) < ceiling, "{next}");

        // A port nobody holds is returned as is (retry against races with
        // other tests grabbing ephemeral ports).
        let free = (0..5).find_map(|_| {
            let probe = TcpListener::bind((Ipv4Addr::UNSPECIFIED, 0)).ok()?;
            let candidate = probe.local_addr().ok()?.port();
            drop(probe);
            (allocate_port(candidate) == Some(candidate)).then_some(candidate)
        });
        assert!(free.is_some(), "no free ephemeral port could be reproduced");
    }

    #[test]
    fn overflow_near_the_top_of_the_range_returns_none_or_valid() {
        if let Some(port) = allocate_port(u16::MAX) {
            assert_eq!(port, u16::MAX);
        }
    }
}
