//! A run keeps the owned stack's chain tip fresh, so the daemon never
//! drains under it, and mines nothing anywhere else.
use std::sync::atomic::{AtomicUsize, Ordering};

use super::maintenance::Advertising;
use super::*;
use crate::run::chain_tip::{CHECK, Kept, Patience, STALE_AFTER, TipError, keep_fresh, refresh};
use crate::stack::{ChainTip, StackError, Tip};

const NOW: u64 = 1_792_069_200;
const MINUTE: u64 = 60;

/// A chain whose newest block is `age_secs` old at [`NOW`] (or at the real
/// clock, for whole runs), and which counts the blocks it mined.
struct FakeChain {
    chain: &'static str,
    age_secs: u64,
    now: u64,
    unreadable: bool,
    mining_fails: bool,
    mined: AtomicUsize,
}

impl FakeChain {
    fn aged(minutes: u64) -> Self {
        Self {
            chain: "regtest",
            age_secs: minutes * MINUTE,
            now: NOW,
            unreadable: false,
            mining_fails: false,
            mined: AtomicUsize::new(0),
        }
    }

    /// Aged against the real clock, as a whole run reads it.
    fn aged_now(minutes: u64) -> Self {
        Self {
            now: now_unix(),
            ..Self::aged(minutes)
        }
    }

    fn mined(&self) -> usize {
        self.mined.load(Ordering::SeqCst)
    }
}

#[async_trait]
impl ChainTip for FakeChain {
    async fn tip(&self) -> Result<Tip, StackError> {
        if self.unreadable {
            return Err(StackError::Configuration(
                "service \"bitcoind\" is not running".into(),
            ));
        }
        Ok(Tip {
            chain: self.chain.into(),
            height: 162,
            time: self.now - self.age_secs,
        })
    }

    async fn mine_block(&self) -> Result<(), StackError> {
        if self.mining_fails {
            return Err(StackError::Configuration("wallet not loaded".into()));
        }
        self.mined.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

fn now_unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

/// [`ReadyNet`] whose daemon advertises maintenance mode for its first
/// `drained` info probes, then leaves it; counts the probes. The probe
/// numbered `unheard`, if any, finds no info event at all.
struct DrainsFor {
    drained: usize,
    unheard: Option<usize>,
    probes: AtomicUsize,
}

impl DrainsFor {
    fn probes(drained: usize) -> Self {
        Self {
            drained,
            unheard: None,
            probes: AtomicUsize::new(0),
        }
    }
}

#[async_trait]
impl NetProbes for DrainsFor {
    async fn local_port(&self, port: u16) -> PortState {
        ReadyNet.local_port(port).await
    }

    async fn daemon(&self, relays: &[String], pubkey: PublicKey) -> DaemonProbe {
        let probe = self.probes.fetch_add(1, Ordering::SeqCst);
        let answer = Advertising(if probe < self.drained {
            "true"
        } else {
            "false"
        })
        .daemon(relays, pubkey)
        .await;
        if self.unheard == Some(probe) {
            return DaemonProbe {
                info: None,
                ..answer
            };
        }
        answer
    }

    async fn wallet(
        &self,
        label: &str,
        wallet: &WalletConfig,
    ) -> Result<WalletProbe, WalletProbeError> {
        ReadyNet.wallet(label, wallet).await
    }
}

/// Long enough for a loaded machine to see a daemon come back; a test
/// that sees it returns at once.
const BRIEF: Patience = Patience {
    deadline: Duration::from_secs(5),
    poll: Duration::from_millis(5),
};

/// For a daemon that never comes back.
const SHORT: Patience = Patience {
    deadline: Duration::from_millis(100),
    poll: Duration::from_millis(5),
};

#[tokio::test]
async fn a_tip_younger_than_an_hour_is_left_alone() {
    // Arrange
    let chain = FakeChain::aged(59);

    // Act
    let kept = keep_fresh(&chain, NOW).await.unwrap();

    // Assert
    assert_eq!(
        kept,
        Kept::Fresh {
            age: Duration::from_secs(59 * MINUTE)
        }
    );
    assert_eq!(chain.mined(), 0);
}

#[tokio::test]
async fn a_tip_older_than_an_hour_gets_exactly_one_block() {
    // Arrange
    let chain = FakeChain::aged(72);

    // Act
    let kept = keep_fresh(&chain, NOW).await.unwrap();

    // Assert
    assert_eq!(
        kept,
        Kept::Mined {
            age: Duration::from_secs(72 * MINUTE)
        }
    );
    assert!(Duration::from_secs(72 * MINUTE) > STALE_AFTER);
    assert_eq!(chain.mined(), 1);
}

#[tokio::test]
async fn nothing_is_mined_on_a_chain_that_is_not_regtest() {
    // Arrange
    let chain = FakeChain {
        chain: "main",
        ..FakeChain::aged(180)
    };

    // Act
    let error = keep_fresh(&chain, NOW).await.unwrap_err();

    // Assert
    assert!(matches!(&error, TipError::NotRegtest(net) if net == "main"));
    assert!(error.to_string().contains("only on regtest"), "{error}");
    assert_eq!(chain.mined(), 0);
}

#[tokio::test]
async fn a_tip_that_cannot_be_read_or_refreshed_fails_the_check_and_says_why() {
    for (chain, expected) in [
        (
            FakeChain {
                unreadable: true,
                ..FakeChain::aged(10)
            },
            "cannot read the chain tip: stack is not configured: service \"bitcoind\" is not \
             running; `mortsom stack status` says whether bitcoind is up",
        ),
        (
            FakeChain {
                mining_fails: true,
                ..FakeChain::aged(2 * 60 + 5)
            },
            "the chain tip is 2h 05m old and mining a block failed: \
             stack is not configured: wallet not loaded",
        ),
    ] {
        // Act
        let (status, detail) = refresh(&world(false).config, &chain, None, NOW, BRIEF).await;

        // Assert
        assert_eq!(status, CheckStatus::Fail);
        assert_eq!(detail, expected);
    }
}

#[tokio::test]
async fn a_fresh_tip_passes_without_asking_the_daemon_anything() {
    // Arrange
    let chain = FakeChain::aged(12);
    let net = DrainsFor::probes(usize::MAX);

    // Act
    let (status, detail) = refresh(&world(false).config, &chain, Some(&net), NOW, BRIEF).await;

    // Assert
    assert_eq!(status, CheckStatus::Pass);
    assert_eq!(detail, "the newest block is 12m 00s old");
    assert_eq!(net.probes.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn a_block_mined_for_a_daemon_that_was_not_draining_needs_no_wait() {
    // Arrange
    let chain = FakeChain::aged(75);
    let net = DrainsFor::probes(0);

    // Act
    let (status, detail) = refresh(&world(false).config, &chain, Some(&net), NOW, BRIEF).await;

    // Assert
    assert_eq!(status, CheckStatus::Pass);
    assert_eq!(detail, "mined a block: the chain tip was 1h 15m old");
    assert_eq!(net.probes.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn after_mining_the_run_waits_for_a_drained_daemon_to_come_back() {
    // Arrange
    let chain = FakeChain::aged(3 * 60);
    let net = DrainsFor::probes(3);

    // Act
    let (status, detail) = refresh(&world(false).config, &chain, Some(&net), NOW, BRIEF).await;

    // Assert
    assert_eq!(status, CheckStatus::Pass);
    assert!(
        detail.starts_with(
            "mined a block: the chain tip was 3h 00m old; the daemon left maintenance mode"
        ),
        "{detail}"
    );
    assert_eq!(chain.mined(), 1);
}

#[tokio::test]
async fn a_daemon_still_draining_after_the_wait_is_left_to_the_preflight() {
    // Arrange
    let chain = FakeChain::aged(3 * 60);
    let net = DrainsFor::probes(usize::MAX);

    // Act
    let (status, detail) = refresh(&world(false).config, &chain, Some(&net), NOW, SHORT).await;

    // Assert
    assert_eq!(status, CheckStatus::Pass, "the block was mined");
    assert!(
        detail.contains("; the daemon was still in maintenance mode "),
        "{detail}"
    );
}

#[tokio::test]
async fn a_probe_that_hears_no_daemon_does_not_end_the_wait() {
    // Arrange
    let chain = FakeChain::aged(3 * 60);
    let net = DrainsFor {
        unheard: Some(1),
        ..DrainsFor::probes(usize::MAX)
    };

    // Act
    let (_, detail) = refresh(&world(false).config, &chain, Some(&net), NOW, SHORT).await;

    // Assert
    assert!(
        detail.contains("; the daemon was still in maintenance mode "),
        "{detail}"
    );
    assert!(net.probes.load(Ordering::SeqCst) > 2);
}

/// Runs `options` with `chain` as the stack's chain.
async fn run_with_chain(world: &World, options: RunOptions, chain: &FakeChain) -> RunSummary {
    let adapter = Arc::new(FakeAdapter::full());
    let clients = FakeClients(adapter.flags.clone());
    let env = env_for(world, false);
    let wallets = nodes_on(Arc::clone(&adapter.ledger), "regtest");
    let deps = RunDeps {
        desktop_factory: &clients,
        net: Some(&ReadyNet),
        chain: Some(chain),
        docker: Arc::new(FakeDocker::default()),
        probe: Arc::new(AlwaysReady),
        adapter: adapter as Arc<dyn AppAdapter>,
        env: &env,
        wallets: &wallets,
        observer: &FakeObserver::default(),
    };
    run_scenarios(&world.config, &options, &deps).await.unwrap()
}

fn chain_check(summary: &RunSummary) -> Option<&mortsom_common::check::CheckResult> {
    summary
        .checks
        .checks
        .iter()
        .find(|check| check.name == CHECK)
}

/// A run on a stack Mortsom owns, kept in `world`.
fn owning(world: &World) -> RunOptions {
    RunOptions {
        stack_dir: Some(world.dir.path().join("stack")),
        ..opts_in(world, &["happy_sell"])
    }
}

#[tokio::test]
async fn a_run_on_an_owned_stack_with_a_stale_tip_mines_one_block_and_passes() {
    // Arrange
    mortsom_scenarios::link();
    let world = world(false);
    let chain = FakeChain::aged_now(2 * 60 + 30);

    // Act
    let summary = run_with_chain(&world, owning(&world), &chain).await;

    // Assert
    assert!(
        summary.passed,
        "{}\n{:?}",
        summary.checks.render(),
        summary.results
    );
    assert_eq!(chain.mined(), 1);
    let check = chain_check(&summary).unwrap();
    assert_eq!(check.status, CheckStatus::Pass);
    assert!(
        check
            .detail
            .starts_with("mined a block: the chain tip was 2h 30m old"),
        "{}",
        check.detail
    );
}

#[tokio::test]
async fn a_run_never_mines_on_a_stack_it_does_not_own() {
    mortsom_scenarios::link();
    let world = world(false);
    let unrecorded_dir = opts_in(&world, &["happy_sell"]);
    let no_stack = RunOptions {
        stack: None,
        ..owning(&world)
    };
    for options in [unrecorded_dir, no_stack] {
        // Arrange
        let chain = FakeChain::aged_now(3 * 60);

        // Act
        let summary = run_with_chain(&world, options, &chain).await;

        // Assert
        assert_eq!(chain.mined(), 0);
        assert!(
            chain_check(&summary).is_none(),
            "{}",
            summary.checks.render()
        );
    }
}

#[tokio::test]
async fn a_tip_that_cannot_be_read_stops_the_run_before_any_client_starts() {
    // Arrange
    mortsom_scenarios::link();
    let world = world(false);
    let chain = FakeChain {
        unreadable: true,
        ..FakeChain::aged_now(10)
    };

    // Act
    let summary = run_with_chain(&world, owning(&world), &chain).await;

    // Assert
    assert!(!summary.passed);
    assert!(summary.actors.is_empty() && summary.farm_teardown.is_empty());
    assert_eq!(chain_check(&summary).unwrap().status, CheckStatus::Fail);
    let [result] = summary.results.as_slice() else {
        panic!("{:?}", summary.results);
    };
    assert!(
        matches!(
            result.outcome,
            Outcome::Failed {
                category: mortsom_common::FailureCategory::EnvironmentMisconfiguration,
                ..
            }
        ),
        "{:?}",
        result.outcome
    );
}
