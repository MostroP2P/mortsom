#![allow(clippy::unwrap_used, clippy::expect_used)]

use super::*;
use crate::process::{ProcessFailure, ProcessOutput};
use crate::testing::{ScriptedCli, document};
use mortsom_common::Secret;

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
    let diagnostics = client.diagnostics();
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

#[test]
fn a_temporary_root_is_released_when_the_session_ends() {
    let (_, client) = launched();
    let root = client.root().unwrap();

    client.close();

    assert!(client.root().is_none());
    assert!(!root.exists());
}

/// A CLI that takes `delay` to answer every command with an empty `ok`.
struct SlowCli {
    delay: Duration,
}

#[async_trait::async_trait]
impl CliProcess for SlowCli {
    async fn run(
        &self,
        request: &ProcessRequest,
        _timeout: Duration,
    ) -> Result<ProcessOutput, ProcessFailure> {
        tokio::time::sleep(self.delay).await;
        let command = crate::testing::command_of(&request.args);
        Ok(document(&command, "ok", &serde_json::json!({}), None, 0))
    }
}

fn retained(process: Arc<dyn CliProcess>) -> (tempfile::TempDir, PathBuf, Arc<CliClient>) {
    let parent = tempfile::tempdir().unwrap();
    let root = parent.path().canonicalize().unwrap().join("alice");
    let client = CliClient::new(config(), process).with_storage(ActorStorage::Fresh(root.clone()));
    client.start_fresh().unwrap();
    (parent, root, Arc::new(client))
}

#[tokio::test]
async fn a_caller_that_gives_up_leaves_no_marker_and_no_overlap() {
    let (_parent, root, client) = retained(Arc::new(SlowCli {
        delay: Duration::from_millis(300),
    }));

    let gave_up = tokio::time::timeout(
        Duration::from_millis(50),
        client.invoke(Call::local(&["identity", "show"])),
    )
    .await;
    let busy = client.start_fresh();
    let next = client.invoke(Call::local(&["identity", "show"])).await;

    assert!(gave_up.is_err(), "the caller should have timed out");
    assert!(matches!(busy, Err(CliError::Storage(_))), "{busy:?}");
    assert!(next.is_ok(), "{next:?}");
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
    cli.raw("order.new", Err(ProcessFailure::Signaled));

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

    let stderr = &client.diagnostics()[1].1;
    assert!(!stderr.contains("lnbcrt"), "{stderr}");
    assert!(!stderr.contains("someone@example.com"), "{stderr}");
}
