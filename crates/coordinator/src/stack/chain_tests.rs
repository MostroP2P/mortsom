#![allow(clippy::unwrap_used)]
//! The chain tip against a scripted `bitcoin-cli`: no Docker, no bitcoind.
use std::path::Path;
use std::sync::Mutex;

use mortsom_ui_driver::process::{ChildProcess, Command, Output, ProcessError};

use super::*;

const BEST: &str = "0f9188f13cb7b2c71f2a335e3a4fc328bf5beb436012afca590b1a11466e2206";

/// A `bitcoind` that answers the calls the chain tip makes, and records
/// every command it was handed.
struct ScriptedBitcoind {
    chain: &'static str,
    /// Whether `getblockchaininfo` fails, as when the container is gone.
    down: bool,
    calls: Mutex<Vec<String>>,
}

impl ScriptedBitcoind {
    fn on(chain: &'static str) -> Self {
        Self {
            chain,
            down: false,
            calls: Mutex::new(Vec::new()),
        }
    }

    fn calls(&self) -> Vec<String> {
        self.calls.lock().unwrap().clone()
    }

    fn reply(&self, args: &[String]) -> Output {
        let has = |needle: &str| args.iter().any(|arg| arg == needle);
        let ok = |stdout: String| Output {
            success: true,
            stdout,
            stderr: String::new(),
        };
        if has("getblockchaininfo") && self.down {
            return Output {
                success: false,
                stdout: String::new(),
                stderr: "service \"bitcoind\" is not running".into(),
            };
        }
        if has("getblockchaininfo") {
            return ok(serde_json::json!({
                "chain": self.chain, "blocks": 162, "bestblockhash": BEST,
            })
            .to_string());
        }
        if has("getblockheader") {
            return ok(serde_json::json!({
                "hash": BEST, "height": 162, "time": 1_792_069_200_u64,
            })
            .to_string());
        }
        if has("getnewaddress") {
            return ok("bcrt1qmine\n".into());
        }
        ok(String::new())
    }
}

#[async_trait]
impl CommandRunner for ScriptedBitcoind {
    async fn run(&self, command: &Command, _timeout: Duration) -> Result<Output, ProcessError> {
        self.calls.lock().unwrap().push(command.display());
        Ok(self.reply(&command.args))
    }

    async fn spawn(
        &self,
        _command: &Command,
        _log_file: Option<&Path>,
    ) -> Result<Box<dyn ChildProcess>, ProcessError> {
        unreachable!("the chain tip never spawns")
    }
}

fn layout() -> (tempfile::TempDir, Layout) {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("compose.yml"), "services: {}\n").unwrap();
    let layout = Layout::new(dir.path()).unwrap();
    (dir, layout)
}

#[tokio::test]
async fn reads_the_newest_block_time_from_its_header() {
    // Arrange
    let (_dir, layout) = layout();
    let bitcoind = ScriptedBitcoind::on("regtest");

    // Act
    let tip = ComposeChain::new(&layout, &bitcoind).tip().await.unwrap();

    // Assert
    assert_eq!(
        tip,
        Tip {
            chain: "regtest".into(),
            height: 162,
            time: 1_792_069_200,
        }
    );
    let header = bitcoind
        .calls()
        .into_iter()
        .find(|call| call.contains("getblockheader"))
        .unwrap();
    assert!(
        header.ends_with(&format!("getblockheader {BEST}")),
        "{header}"
    );
}

#[tokio::test]
async fn mines_one_block_to_the_miner_wallet() {
    // Arrange
    let (_dir, layout) = layout();
    let bitcoind = ScriptedBitcoind::on("regtest");

    // Act
    ComposeChain::new(&layout, &bitcoind)
        .mine_block()
        .await
        .unwrap();

    // Assert
    let calls = bitcoind.calls();
    let generated: Vec<_> = calls
        .iter()
        .filter(|call| call.contains("generatetoaddress"))
        .collect();
    assert_eq!(generated.len(), 1, "{calls:?}");
    assert!(
        generated[0].ends_with("-rpcwallet=mortsom generatetoaddress 1 bcrt1qmine"),
        "{}",
        generated[0]
    );
    assert!(
        calls
            .iter()
            .all(|call| call.contains("bitcoin-cli -regtest")),
        "{calls:?}"
    );
}

#[tokio::test]
async fn a_bitcoind_that_does_not_answer_is_an_error() {
    // Arrange
    let (_dir, layout) = layout();
    let bitcoind = ScriptedBitcoind {
        down: true,
        ..ScriptedBitcoind::on("regtest")
    };

    // Act
    let error = ComposeChain::new(&layout, &bitcoind)
        .tip()
        .await
        .unwrap_err();

    // Assert
    assert!(error.to_string().contains("not running"), "{error}");
}

#[test]
fn refuses_replies_that_are_not_what_bitcoind_prints() {
    assert!(chain_info("error code: -28 Loading block index").is_err());
    assert!(
        chain_info(r#"{"chain":"regtest","bestblockhash":"00ab"}"#).is_err(),
        "a short hash"
    );
    assert!(
        chain_info(&format!(
            r#"{{"chain":"regtest","bestblockhash":"{}"}}"#,
            "zz".repeat(32)
        ))
        .is_err(),
        "a hash that is not hex"
    );
    assert!(header_time(r#"{"height":1}"#).is_err());
}
