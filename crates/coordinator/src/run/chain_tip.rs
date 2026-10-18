//! Keeping the owned stack's chain tip fresh, so mostrod never drains
//! mid-run.
//!
//! LND reports `synced_to_chain = false` about two hours after the newest
//! block, and ten minutes later mostrod enters maintenance mode (see
//! `maintenance`). `stack up` mines only while it builds, so before each run
//! on a stack Mortsom owns, a tip older than [`STALE_AFTER`] gets one block
//! more; one block barely moves a CLTV, so that is safe between runs. A
//! selection is a sequence of runs of one scenario each (`cli::run`), so
//! this happens once per scenario, before the preflight, which then judges
//! the stack as the run has left it.
//!
//! Nothing is mined on a stack the run does not own, nor on a chain that
//! bitcoind itself does not call regtest.
use std::time::Duration;

use mortsom_common::check::CheckStatus;
use mortsom_scenario_kit::waits::wait_until;
use thiserror::Error;
use tracing::{info, warn};

use super::{RunDeps, RunOptions, maintenance};
use crate::config::Config;
use crate::doctor::NetProbes;
use crate::stack::{ChainTip, StackError};

/// The preflight check that says what keeping the tip fresh did.
pub(super) const CHECK: &str = "chain-tip";

/// A tip older than this gets a block: half the two hours after which LND
/// calls itself unsynced, so a whole run fits in what is left.
pub(super) const STALE_AFTER: Duration = Duration::from_secs(60 * 60);

/// How long a run waits, after mining, for the daemon to leave maintenance
/// mode, and how often it looks. mostrod wants LND synced for 300 s, asks
/// LND every 15 s and republishes its info event as soon as maintenance
/// changes; the rest is room for a slow relay.
pub(super) const PATIENCE: Patience = Patience {
    deadline: Duration::from_secs(7 * 60),
    poll: Duration::from_secs(10),
};

/// A bounded wait: its deadline and its poll interval.
#[derive(Debug, Clone, Copy)]
pub(super) struct Patience {
    pub(super) deadline: Duration,
    pub(super) poll: Duration,
}

/// What keeping the tip fresh did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Kept {
    /// The newest block was `age` old, recent enough.
    Fresh { age: Duration },
    /// The newest block was `age` old, so one more was mined.
    Mined { age: Duration },
}

/// Why the tip could not be kept fresh.
#[derive(Debug, Error)]
pub(super) enum TipError {
    /// bitcoind did not say what its newest block is.
    #[error("cannot read the chain tip: {0}; `mortsom stack status` says whether bitcoind is up")]
    Unreadable(StackError),
    /// bitcoind runs another network; holds its name.
    #[error("bitcoind runs {0:?}, not regtest; Mortsom mines only on regtest")]
    NotRegtest(String),
    /// The tip was stale and the block that would refresh it was not mined.
    #[error("the chain tip is {} old and mining a block failed: {error}", age_text(.age))]
    Mining { age: Duration, error: StackError },
}

/// The chain of the stack this run owns, and `None` when it owns none: only
/// a Mortsom-owned stack ever has a block mined by a run.
pub(super) fn owned<'a>(options: &RunOptions, deps: &RunDeps<'a>) -> Option<&'a dyn ChainTip> {
    options.stack.as_ref().and(options.stack_dir.as_ref())?;
    deps.chain
}

/// Mines one block when the newest one is older than [`STALE_AFTER`] at
/// `now_unix`, and never on anything but regtest.
pub(super) async fn keep_fresh(chain: &dyn ChainTip, now_unix: u64) -> Result<Kept, TipError> {
    let tip = chain.tip().await.map_err(TipError::Unreadable)?;
    if tip.chain != "regtest" {
        return Err(TipError::NotRegtest(tip.chain));
    }
    // A tip dated after `now_unix` (host and container clocks disagree)
    // counts as just mined.
    let age = Duration::from_secs(now_unix.saturating_sub(tip.time));
    if age <= STALE_AFTER {
        return Ok(Kept::Fresh { age });
    }
    chain
        .mine_block()
        .await
        .map_err(|error| TipError::Mining { age, error })?;
    Ok(Kept::Mined { age })
}

/// Keeps the tip fresh and says how, as the [`CHECK`] the preflight reads:
/// a tip that cannot be read or refreshed fails it, so the run never starts
/// on a chain that may go stale under it. Having mined, it waits out a
/// daemon that already drained (see [`wait_out_maintenance`]).
pub(super) async fn refresh(
    config: &Config,
    chain: &dyn ChainTip,
    net: Option<&dyn NetProbes>,
    now_unix: u64,
    patience: Patience,
) -> (CheckStatus, String) {
    let detail = match keep_fresh(chain, now_unix).await {
        Ok(Kept::Fresh { age }) => format!("the newest block is {} old", age_text(&age)),
        Ok(Kept::Mined { age }) => {
            let mined = format!("mined a block: the chain tip was {} old", age_text(&age));
            match wait_out_maintenance(config, net, patience).await {
                Some(waited) => format!("{mined}; {waited}"),
                None => mined,
            }
        }
        Err(error) => {
            warn!(%error, "cannot keep the chain tip fresh");
            return (CheckStatus::Fail, error.to_string());
        }
    };
    info!(%detail, "chain tip");
    (CheckStatus::Pass, detail)
}

/// A daemon that drained because the tip went stale stays drained until
/// LND has been synced for five minutes. Having just mended the cause, the
/// run waits for that, bounded, rather than refuse a stack about to work;
/// if the daemon is still draining after it, the preflight refuses the run
/// and says why. What it saw, or `None` when the daemon was not seen
/// draining (the preflight reads it again either way).
async fn wait_out_maintenance(
    config: &Config,
    net: Option<&dyn NetProbes>,
    patience: Patience,
) -> Option<String> {
    let (net, key) = (net?, config.mobile.mostro_pubkey()?);
    // `None` when no info event was heard: that says nothing either way.
    let draining = move || async move {
        net.daemon(&config.mostro.relays, key)
            .await
            .info
            .as_ref()
            .map(maintenance::advertised)
    };
    if draining().await != Some(true) {
        return None;
    }
    // On the terminal too: the run is silent for minutes otherwise.
    warn!(
        "the daemon is in maintenance mode; waiting up to {} for it to leave",
        age_text(&patience.deadline)
    );
    let started = tokio::time::Instant::now();
    let left = wait_until(patience.deadline, patience.poll, || async {
        (draining().await == Some(false)).then_some(())
    })
    .await;
    Some(match left {
        Some(()) => format!(
            "the daemon left maintenance mode {} later",
            age_text(&started.elapsed())
        ),
        None => format!(
            "the daemon was still in maintenance mode {} later",
            age_text(&patience.deadline)
        ),
    })
}

/// `1h 12m`, `5m 10s`, `40s`: precise enough to say how stale a tip was.
fn age_text(age: &Duration) -> String {
    let secs = age.as_secs();
    match (secs / 3600, secs % 3600 / 60, secs % 60) {
        (0, 0, seconds) => format!("{seconds}s"),
        (0, minutes, seconds) => format!("{minutes}m {seconds:02}s"),
        (hours, minutes, _) => format!("{hours}h {minutes:02}m"),
    }
}
