//! The stack's chain as a run needs it: how old its newest block is, and
//! one block more when that is too old.
//!
//! `stack up` mines only while it builds. About two hours after the newest
//! block the LND nodes report `synced_to_chain = false` and mostrod drains,
//! so a run on this stack keeps the tip fresh itself (`run::chain_tip`).
use std::time::Duration;

use async_trait::async_trait;
use mortsom_ui_driver::process::CommandRunner;
use serde::Deserialize;

use super::StackError;
use super::compose::{Compose, require};
use super::layout::Layout;
use super::up;

const STEP_TIMEOUT: Duration = Duration::from_secs(60);

/// The newest block, as bitcoind reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tip {
    /// The network bitcoind runs: `regtest`, `main`, `test`, ...
    pub chain: String,
    /// Height of the newest block.
    pub height: u64,
    /// The newest block's header time, unix seconds.
    pub time: u64,
}

/// The chain of a stack Mortsom owns. A trait so the run is tested
/// without bitcoind.
#[async_trait]
pub trait ChainTip: Send + Sync {
    /// The newest block and the network it is on.
    async fn tip(&self) -> Result<Tip, StackError>;
    /// Mines one block to the miner wallet. Callers mine only on regtest,
    /// which [`ChainTip::tip`] tells them.
    async fn mine_block(&self) -> Result<(), StackError>;
}

/// [`ChainTip`] through `bitcoin-cli` in the stack's `bitcoind` container,
/// the way `stack up` mines.
pub struct ComposeChain<'a> {
    compose: Compose,
    runner: &'a dyn CommandRunner,
}

impl<'a> ComposeChain<'a> {
    /// The chain of the Compose project in `layout`.
    pub fn new(layout: &Layout, runner: &'a dyn CommandRunner) -> Self {
        Self {
            compose: Compose::new(layout, None),
            runner,
        }
    }
}

#[async_trait]
impl ChainTip for ComposeChain<'_> {
    async fn tip(&self) -> Result<Tip, StackError> {
        let info = require(
            self.runner,
            &self.compose.bitcoin_cli(&["getblockchaininfo"]),
            STEP_TIMEOUT,
        )
        .await?;
        let (chain, best) = chain_info(&info.stdout)?;
        let header = require(
            self.runner,
            &self.compose.bitcoin_cli(&["getblockheader", &best]),
            STEP_TIMEOUT,
        )
        .await?;
        let (height, time) = header_time(&header.stdout)?;
        Ok(Tip {
            chain,
            height,
            time,
        })
    }

    async fn mine_block(&self) -> Result<(), StackError> {
        let miner = up::miner_address(self.runner, &self.compose).await?;
        up::mine(self.runner, &self.compose, &miner, 1).await
    }
}

#[derive(Deserialize)]
struct ChainInfo {
    chain: String,
    bestblockhash: String,
}

#[derive(Deserialize)]
struct Header {
    height: u64,
    time: u64,
}

/// The network and best block hash from `getblockchaininfo`. The hash is
/// handed back to `bitcoin-cli`, so anything but 64 hex digits is refused.
fn chain_info(raw: &str) -> Result<(String, String), StackError> {
    let info: ChainInfo = parse("getblockchaininfo", raw)?;
    let hash = &info.bestblockhash;
    if hash.len() != 64 || !hash.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(StackError::Configuration(format!(
            "getblockchaininfo: {hash:?} is not a block hash"
        )));
    }
    Ok((info.chain, info.bestblockhash))
}

/// Height and header time from `getblockheader`.
fn header_time(raw: &str) -> Result<(u64, u64), StackError> {
    let header: Header = parse("getblockheader", raw)?;
    Ok((header.height, header.time))
}

fn parse<T: for<'de> Deserialize<'de>>(what: &str, raw: &str) -> Result<T, StackError> {
    serde_json::from_str(raw.trim())
        .map_err(|error| StackError::Configuration(format!("{what}: unreadable reply: {error}")))
}

#[cfg(test)]
#[path = "chain_tests.rs"]
mod tests;
