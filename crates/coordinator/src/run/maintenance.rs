//! The daemon's maintenance ("drain") mode, as its info event advertises it.
//!
//! mostrod v0.20 turns maintenance on by itself once its LND node has
//! reported `synced_to_chain = false` for ten minutes, and off once LND has
//! been synced again for five. While it is on, the daemon rejects new
//! orders and takes and skips bond payout cycles and failed-payment
//! retries, so every scenario that creates an order fails as if the app
//! were wrong. On Mortsom's regtest stack the usual cause is a stale chain
//! tip: nothing mines after `stack up`, and LND calls itself unsynced about
//! two hours after the newest block.
use mortsom_common::check::CheckStatus;
use mortsom_environment::daemon::DaemonInfo;

/// The info event's tag: `"true"` or `"false"` on every mostrod that has
/// maintenance mode; older daemons do not send it.
pub(super) const TAG: &str = "maintenance_mode";

/// What a draining daemon does not do, the usual cause on this stack, and
/// how to bring it back.
const WHY_AND_FIX: &str = "it rejects new orders and takes, and skips bond payout cycles and \
     payment retries. On a regtest stack the usual cause is a stale chain tip: LND reports \
     synced_to_chain=false about two hours after the newest block. Mine a block (runbook 2.5) and \
     wait five minutes: mostrod leaves maintenance mode once LND has been synced for 300 s";

/// Whether `info` says the daemon is in maintenance mode.
pub(super) fn advertised(info: &DaemonInfo) -> bool {
    info.tag(TAG) == Some("true")
}

/// Why a scenario that failed while the daemon advertised maintenance mode
/// failed for the environment, not for the app.
pub(super) fn blame() -> String {
    format!("the daemon was in maintenance mode ({TAG}=true): {WHY_AND_FIX}")
}

/// The `daemon-maintenance` preflight check: a run never starts against a
/// daemon that would refuse its orders. A daemon that does not send the
/// tag predates maintenance mode and passes; a value that is neither
/// `true` nor `false` cannot show the daemon accepts orders, and fails.
pub(super) fn check(info: &DaemonInfo) -> (CheckStatus, String) {
    match info.tag(TAG) {
        Some("false") => (
            CheckStatus::Pass,
            "the daemon is not in maintenance mode".into(),
        ),
        None => (
            CheckStatus::Pass,
            format!("the daemon sends no {TAG} tag: it predates maintenance mode"),
        ),
        Some("true") => (
            CheckStatus::Fail,
            format!("the daemon is in maintenance mode ({TAG}=true): {WHY_AND_FIX}"),
        ),
        Some(other) => (
            CheckStatus::Fail,
            format!("the daemon advertises {TAG}={other:?}, neither true nor false"),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info(maintenance: Option<&str>) -> DaemonInfo {
        DaemonInfo {
            pubkey: "daemon".into(),
            created_at: 0,
            tags: maintenance
                .map(|value| (TAG.to_owned(), value.to_owned()))
                .into_iter()
                .collect(),
        }
    }

    #[test]
    fn a_daemon_in_maintenance_mode_fails_the_preflight_naming_cause_and_fix() {
        // Act
        let (status, detail) = check(&info(Some("true")));

        // Assert
        assert_eq!(status, CheckStatus::Fail);
        assert!(detail.contains("maintenance_mode=true"), "{detail}");
        assert!(detail.contains("stale chain tip"), "{detail}");
        assert!(detail.contains("synced_to_chain=false"), "{detail}");
        assert!(detail.contains("Mine a block"), "{detail}");
        assert!(detail.contains("wait five minutes"), "{detail}");
    }

    #[test]
    fn a_daemon_out_of_maintenance_mode_passes() {
        // Act
        let (status, detail) = check(&info(Some("false")));

        // Assert
        assert_eq!(status, CheckStatus::Pass, "{detail}");
    }

    #[test]
    fn a_daemon_that_predates_maintenance_mode_passes() {
        // Act
        let (status, detail) = check(&info(None));

        // Assert
        assert_eq!(status, CheckStatus::Pass, "{detail}");
        assert!(detail.contains("predates"), "{detail}");
    }

    #[test]
    fn a_maintenance_value_that_is_neither_true_nor_false_fails() {
        // Act
        let (status, detail) = check(&info(Some("yes")));

        // Assert
        assert_eq!(status, CheckStatus::Fail);
        assert!(detail.contains("\"yes\""), "{detail}");
    }

    #[test]
    fn only_true_counts_as_advertised() {
        assert!(advertised(&info(Some("true"))));
        for value in [Some("false"), Some("yes"), None] {
            assert!(!advertised(&info(value)), "{value:?}");
        }
    }
}
