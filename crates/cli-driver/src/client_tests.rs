#![allow(clippy::unwrap_used, clippy::expect_used)]

use super::*;
use crate::process::{ProcessFailed, ProcessFailure, ProcessOutput};
use crate::testing::{ScriptedCli, SlowCli, command_of, document};
use mortsom_common::Secret;
use std::sync::atomic::{AtomicUsize, Ordering};

fn config() -> CliConfig {
    CliConfig {
        binary: "/opt/mostro-cli".into(),
        node: NodeTarget {
            mostro_pubkey: "ab".repeat(32),
            relays: vec!["ws://127.0.0.1:7500".into()],
            reply_timeout_secs: None,
        },
        timeout: Duration::from_secs(5),
        path_env: Some("/usr/bin".into()),
    }
}

fn launched() -> (Arc<ScriptedCli>, CliClient) {
    let cli = Arc::new(ScriptedCli::default());
    let client = CliClient::new(config(), cli.clone());
    client.start_fresh().unwrap();
    (cli, client)
}

#[tokio::test]
async fn each_invocation_sees_only_the_actors_private_directories() {
    let (cli, client) = launched();
    cli.ok("identity.show", serde_json::json!({"identity_pubkey": "a"}));

    client
        .invoke(Call::local(&["identity", "show"]))
        .await
        .unwrap();

    let seen = &cli.seen()[0];
    let root = client.root().unwrap();
    assert_eq!(
        seen.env["MCLI_DIR"],
        root.join("mcli").display().to_string()
    );
    assert_eq!(seen.env["HOME"], root.join("home").display().to_string());
    assert_eq!(seen.env["PATH"], "/usr/bin");
    assert_eq!(seen.env.len(), 3);
    assert_eq!(seen.stdin, None);
}

#[tokio::test]
async fn the_invoice_reaches_standard_input_and_never_the_transcript() {
    let (cli, client) = launched();
    cli.ok("order.add-invoice", serde_json::json!({"result": "x"}));
    let invoice = Secret::new("lnbcrt1payout");

    client
        .invoke(
            Call::node(&["order", "add-invoice"])
                .arg("order", "o1")
                .invoice(&invoice)
                .confirmed(),
        )
        .await
        .unwrap();

    assert_eq!(cli.seen()[0].stdin.as_deref(), Some("lnbcrt1payout"));
    assert!(!client.take_transcript().contains("lnbcrt1payout"));
}

#[tokio::test]
async fn a_refusal_an_uncertain_write_and_a_broken_answer_are_told_apart() {
    let (cli, client) = launched();
    cli.fail("order.take-sell", "error", "node_refused");
    cli.fail("order.new", "uncertain", "reply_timeout");
    cli.raw(
        "order.release",
        Ok(ProcessOutput {
            code: Some(0),
            stdout: "Released!".into(),
            stderr: "warning\n".into(),
        }),
    );

    let refused = client.invoke(Call::node(&["order", "take-sell"])).await;
    let uncertain = client.invoke(Call::node(&["order", "new"])).await;
    let broken = client.invoke(Call::node(&["order", "release"])).await;

    assert!(matches!(refused, Err(CliError::Refused { ref code, .. }) if code == "node_refused"));
    assert!(matches!(uncertain, Err(CliError::Uncertain { .. })));
    assert!(matches!(broken, Err(CliError::Contract { .. })));
    let diagnostics = client.diagnostics().await;
    assert_eq!(diagnostics.len(), 2);
    assert_eq!(diagnostics[1].1, "warning\n");
    assert_eq!(client.take_transcript().lines().count(), 3);
}

#[tokio::test]
async fn an_answer_for_another_command_breaks_the_contract() {
    let (cli, client) = launched();
    cli.raw(
        "order.info",
        Ok(document(
            "order.list",
            "ok",
            &serde_json::json!({}),
            None,
            0,
        )),
    );

    let error = client
        .invoke(Call::node(&["order", "info"]))
        .await
        .unwrap_err();

    assert!(matches!(error, CliError::Contract { .. }), "{error}");
}

#[tokio::test]
async fn an_invocation_that_never_ends_is_a_process_failure() {
    let (_, client) = launched();

    let error = client
        .invoke(Call::node(&["order", "info"]))
        .await
        .unwrap_err();

    assert!(matches!(error, CliError::Process { .. }), "{error}");
    assert_eq!(
        error.category(),
        mortsom_common::FailureCategory::TestInfrastructure
    );
}

#[tokio::test]
async fn a_fresh_start_empties_the_store_and_a_relaunch_keeps_it() {
    let (_, client) = launched();
    let root = client.root().unwrap();
    std::fs::write(root.join("mcli").join("mcli.db"), b"identity").unwrap();

    client.ensure_open().unwrap();
    assert!(root.join("mcli").join("mcli.db").exists());
    client.start_fresh().unwrap();

    assert!(!root.join("mcli").join("mcli.db").exists());
    assert!(root.join("home").is_dir());
}

#[tokio::test]
async fn retained_storage_records_a_stopped_runtime_after_each_invocation() {
    let parent = tempfile::tempdir().unwrap();
    let root = parent.path().canonicalize().unwrap().join("alice");
    let cli = Arc::new(ScriptedCli::default());
    cli.ok("identity.show", serde_json::json!({"identity_pubkey": "a"}));
    let client =
        CliClient::new(config(), cli.clone()).with_storage(ActorStorage::Fresh(root.clone()));
    client.start_fresh().unwrap();

    client
        .invoke(Call::local(&["identity", "show"]))
        .await
        .unwrap();

    assert!(root.join("mcli").is_dir());
    mortsom_desktop_driver::storage::ensure_stopped(&root).unwrap();
}

#[test]
fn protected_recovery_storage_is_never_cleared() {
    let parent = tempfile::tempdir().unwrap();
    let root = parent.path().join("bob");
    let cli = Arc::new(ScriptedCli::default());
    let client = CliClient::new(config(), cli).with_storage(ActorStorage::Resume(root));

    let error = client.start_fresh().unwrap_err();

    assert!(matches!(error, CliError::Storage(_)), "{error}");
}

/// Retained storage of `platform` at `<tempdir>/bob`, with the marker of a
/// runtime that never recorded its stop.
fn interrupted(platform: Platform) -> (tempfile::TempDir, PathBuf) {
    let parent = tempfile::tempdir().unwrap();
    let root = parent.path().canonicalize().unwrap().join("bob");
    ActorStorage::Fresh(root.clone()).prepare(platform).unwrap();
    let _left_behind = ActiveRuntime::begin(&root).unwrap();
    (parent, root)
}

#[tokio::test]
async fn recovery_reopens_interrupted_storage_once_no_process_uses_it() {
    let (_parent, root) = interrupted(Platform::Cli);
    std::fs::write(root.join("mcli").join("mcli.db"), b"identity").unwrap();
    let cli = Arc::new(ScriptedCli::default());
    cli.ok("identity.show", serde_json::json!({"identity_pubkey": "a"}));
    let client =
        CliClient::new(config(), cli.clone()).with_storage(ActorStorage::Resume(root.clone()));

    client.ensure_open().unwrap();
    client
        .invoke(Call::local(&["identity", "show"]))
        .await
        .unwrap();

    assert_eq!(client.root(), Some(root.clone()));
    assert!(root.join("mcli").join("mcli.db").exists());
    mortsom_desktop_driver::storage::ensure_stopped(&root).unwrap();
}

#[test]
fn recovery_refuses_another_platform_s_storage_and_keeps_its_marker() {
    let (_parent, root) = interrupted(Platform::Linux);
    let cli = Arc::new(ScriptedCli::default());
    let client = CliClient::new(config(), cli).with_storage(ActorStorage::Resume(root.clone()));

    let error = client.ensure_open().unwrap_err();

    assert!(matches!(error, CliError::Storage(_)), "{error}");
    assert!(client.root().is_none());
    assert!(mortsom_desktop_driver::storage::ensure_stopped(&root).is_err());
}

#[test]
fn recovery_refuses_storage_a_live_process_still_uses() {
    let (_parent, root) = interrupted(Platform::Cli);
    let mut live = std::process::Command::new("/bin/sh")
        .args(["-c", "sleep 5"])
        .env("MCLI_DIR", root.join("mcli"))
        .spawn()
        .unwrap();
    let cli = Arc::new(ScriptedCli::default());
    let client = CliClient::new(config(), cli).with_storage(ActorStorage::Resume(root.clone()));

    let opened = client.ensure_open();
    live.kill().unwrap();
    live.wait().unwrap();

    assert!(matches!(opened, Err(CliError::Storage(_))), "{opened:?}");
    assert!(mortsom_desktop_driver::storage::ensure_stopped(&root).is_err());
}

#[tokio::test]
async fn a_temporary_root_is_released_when_the_session_ends() {
    let (_, client) = launched();
    let root = client.root().unwrap();

    client.close().await.unwrap();

    assert!(client.root().is_none());
    assert!(!root.exists());
}

fn retained(process: Arc<dyn CliProcess>) -> (tempfile::TempDir, PathBuf, Arc<CliClient>) {
    let parent = tempfile::tempdir().unwrap();
    let root = parent.path().canonicalize().unwrap().join("alice");
    let client = CliClient::new(config(), process).with_storage(ActorStorage::Fresh(root.clone()));
    client.start_fresh().unwrap();
    (parent, root, Arc::new(client))
}

/// A CLI that takes `delay` to answer with an empty `ok` and records the
/// most invocations it ever ran at once.
#[derive(Debug, Default)]
struct CountingCli {
    delay: Duration,
    running: AtomicUsize,
    most: AtomicUsize,
}

#[async_trait::async_trait]
impl CliProcess for CountingCli {
    async fn run(
        &self,
        request: &ProcessRequest,
        _timeout: Duration,
    ) -> Result<ProcessOutput, ProcessFailed> {
        let now = self.running.fetch_add(1, Ordering::SeqCst) + 1;
        self.most.fetch_max(now, Ordering::SeqCst);
        tokio::time::sleep(self.delay).await;
        self.running.fetch_sub(1, Ordering::SeqCst);
        let command = command_of(&request.args);
        Ok(document(&command, "ok", &serde_json::json!({}), None, 0))
    }
}

#[tokio::test]
async fn a_caller_that_gives_up_leaves_no_marker_and_no_overlap() {
    let cli = Arc::new(CountingCli {
        delay: Duration::from_millis(300),
        ..CountingCli::default()
    });
    let (_parent, root, client) = retained(cli.clone());

    let gave_up = tokio::time::timeout(
        Duration::from_millis(50),
        client.invoke(Call::local(&["identity", "show"])),
    )
    .await;
    let busy = client.start_fresh();
    let (next, other) = tokio::join!(
        client.invoke(Call::local(&["identity", "show"])),
        client.invoke(Call::local(&["identity", "show"])),
    );

    assert!(gave_up.is_err(), "the caller should have timed out");
    assert!(matches!(busy, Err(CliError::Storage(_))), "{busy:?}");
    assert!(next.is_ok(), "{next:?}");
    assert!(other.is_ok(), "{other:?}");
    assert_eq!(cli.most.load(Ordering::SeqCst), 1, "invocations overlapped");
    mortsom_desktop_driver::storage::ensure_stopped(&root).unwrap();
}

/// The evidence of a run that timed out must still show the command it was
/// waiting on: the invocation records its outcome even when no caller is
/// left to read it.
#[tokio::test]
async fn an_invocation_whose_caller_gave_up_still_reaches_the_transcript() {
    let (_parent, _root, client) = retained(Arc::new(SlowCli {
        delay: Duration::from_millis(150),
    }));

    let gave_up = tokio::time::timeout(
        Duration::from_millis(20),
        client.invoke(Call::local(&["identity", "show"])),
    )
    .await;
    assert!(gave_up.is_err(), "the caller should have timed out");
    // The next invocation waits for the abandoned one to finish.
    client
        .invoke(Call::local(&["identity", "show"]))
        .await
        .unwrap();

    let transcript = client.take_transcript();
    assert_eq!(transcript.lines().count(), 2, "{transcript}");
}

/// Abandons one `identity show` that is still running.
async fn give_up_on_one(client: &CliClient) {
    let gave_up = tokio::time::timeout(
        Duration::from_millis(20),
        client.invoke(Call::local(&["identity", "show"])),
    )
    .await;
    assert!(gave_up.is_err(), "the caller should have timed out");
}

/// Teardown must not report a clean stop while a child still uses the
/// actor's storage: closing waits for the invocation a caller abandoned.
#[tokio::test]
async fn closing_waits_for_an_invocation_whose_caller_gave_up() {
    let (_parent, root, client) = retained(Arc::new(SlowCli {
        delay: Duration::from_millis(150),
    }));
    give_up_on_one(&client).await;

    client.close().await.unwrap();

    mortsom_desktop_driver::storage::ensure_stopped(&root).unwrap();
    assert_eq!(client.take_transcript().lines().count(), 1);
}

/// An invocation that outlives its own deadline is a failed shutdown.
#[tokio::test(start_paused = true)]
async fn closing_fails_while_an_invocation_outlives_its_deadline() {
    let (_parent, _root, client) = retained(Arc::new(SlowCli {
        delay: Duration::from_secs(3600),
    }));
    give_up_on_one(&client).await;

    let closed = client.close().await;

    assert!(matches!(closed, Err(CliError::Storage(_))), "{closed:?}");
}

/// The run's evidence is collected before teardown: it must already hold
/// the invocation a caller abandoned.
#[tokio::test]
async fn diagnostics_wait_for_an_invocation_whose_caller_gave_up() {
    let (_parent, _root, client) = retained(Arc::new(SlowCli {
        delay: Duration::from_millis(150),
    }));
    give_up_on_one(&client).await;

    let diagnostics = client.diagnostics().await;

    let transcript = &diagnostics
        .iter()
        .find(|(name, _)| name == "cli-transcript.jsonl")
        .unwrap()
        .1;
    assert!(transcript.contains("identity.show"), "{transcript}");
}

#[tokio::test]
async fn a_crash_is_a_product_failure_and_an_argument_refusal_a_contract_one() {
    let (cli, client) = launched();
    let silent = |code| {
        Ok(ProcessOutput {
            code: Some(code),
            stdout: String::new(),
            stderr: "thread 'main' panicked".into(),
        })
    };
    cli.raw("order.new", silent(101));
    cli.raw("order.new", silent(2));
    cli.raw("order.new", Err(ProcessFailure::Signaled.into()));

    let crashed = client
        .invoke(Call::node(&["order", "new"]))
        .await
        .unwrap_err();
    let refused = client
        .invoke(Call::node(&["order", "new"]))
        .await
        .unwrap_err();
    let killed = client
        .invoke(Call::node(&["order", "new"]))
        .await
        .unwrap_err();

    use mortsom_common::FailureCategory::{AutomationContract, Product, TestInfrastructure};
    assert_eq!(crashed.category(), Product, "{crashed}");
    assert_eq!(refused.category(), AutomationContract, "{refused}");
    assert_eq!(killed.category(), TestInfrastructure, "{killed}");
}

#[tokio::test]
async fn standard_error_keeps_no_invoice() {
    let (cli, client) = launched();
    let invoice = Secret::new("someone@example.com");
    cli.raw(
        "order.add-invoice",
        Ok(ProcessOutput {
            stderr: "sending lnbcrt10n1pqqqqqqqqqqqqqqqqqqqqqqqqqqq for someone@example.com\n"
                .into(),
            ..document("order.add-invoice", "ok", &serde_json::json!({}), None, 0)
        }),
    );

    client
        .invoke(Call::node(&["order", "add-invoice"]).invoice(&invoice))
        .await
        .unwrap();

    let stderr = &client.diagnostics().await[1].1;
    assert!(!stderr.contains("lnbcrt"), "{stderr}");
    assert!(!stderr.contains("someone@example.com"), "{stderr}");
}

/// A child that timed out may have printed why: its standard error reaches
/// the run's evidence, sanitized like any other.
#[tokio::test]
async fn a_timed_out_invocation_keeps_its_standard_error() {
    let (cli, client) = launched();
    cli.raw(
        "order.new",
        Err(ProcessFailed {
            failure: ProcessFailure::Timeout(Duration::from_secs(1)),
            stderr: "waiting on relay for lnbcrt10n1pqqqqqqqqqqqqqqqqqqqqqqqqqqq\n".into(),
        }),
    );

    let error = client
        .invoke(Call::node(&["order", "new"]))
        .await
        .unwrap_err();

    assert!(matches!(error, CliError::Process { .. }), "{error}");
    let diagnostics = client.diagnostics().await;
    let stderr = &diagnostics
        .iter()
        .find(|(name, _)| name == "cli-stderr.log")
        .unwrap()
        .1;
    assert!(stderr.contains("waiting on relay"), "{stderr}");
    assert!(!stderr.contains("lnbcrt"), "{stderr}");
}
