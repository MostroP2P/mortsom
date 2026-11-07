//! Reconciling an interrupted runtime scans `/proc`, which may block (a
//! stat on a hung mount): it runs on the blocking pool, never on a thread
//! of the runtime, and keeps the client busy until it ends.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use super::tests::{config, interrupted};
use super::*;
use crate::testing::ScriptedCli;
use mortsom_desktop_driver::storage::{clear_verified_stop, ensure_stopped};

/// How long a gated reconciliation waits to be released before it fails.
const RELEASE_BOUND: Duration = Duration::from_secs(5);

/// A reconciliation that says it started, then blocks its thread until a
/// task of the runtime releases it, and the future that releases it. Run
/// on the runtime's only thread, the reconciliation would leave that task
/// no chance to run: it fails after a bound instead of hanging the test.
/// Storage without a marker is reconciled at once, as the real one is.
fn gated() -> (
    impl Future<Output = ()>,
    impl Fn(&Path) -> Result<(), CliError> + Send + Sync + 'static,
) {
    let (started, on_start) = tokio::sync::oneshot::channel::<()>();
    let (release, released) = std::sync::mpsc::channel::<()>();
    let started = Mutex::new(Some(started));
    let released = Mutex::new(released);
    let reconcile = move |root: &Path| {
        if ensure_stopped(root).is_ok() {
            return Ok(());
        }
        if let Some(started) = lock(&started).take() {
            started.send(()).unwrap();
        }
        lock(&released)
            .recv_timeout(RELEASE_BOUND)
            .map_err(|_| CliError::Storage("the reconciliation blocked the runtime".into()))?;
        clear_verified_stop(root, Platform::Cli).map_err(storage_error)
    };
    let releasing = async move {
        on_start.await.unwrap();
        release.send(()).unwrap();
    };
    (releasing, reconcile)
}

fn resumed(
    root: &Path,
    reconcile: impl Fn(&Path) -> Result<(), CliError> + Send + Sync + 'static,
) -> CliClient {
    CliClient::new(config(), Arc::new(ScriptedCli::default()))
        .with_storage(ActorStorage::Resume(root.to_path_buf()))
        .reconciling_with(reconcile)
}

#[tokio::test(flavor = "current_thread")]
async fn reopening_interrupted_storage_reconciles_off_the_runtime() {
    let (_parent, root) = interrupted(Platform::Cli);
    let (releasing, reconcile) = gated();
    let client = resumed(&root, reconcile);

    let (opened, ()) = tokio::join!(client.ensure_open(), releasing);

    opened.unwrap();
    assert_eq!(client.root(), Some(root.clone()));
    ensure_stopped(&root).unwrap();
}

#[tokio::test(flavor = "current_thread")]
async fn an_invocation_reconciles_off_the_runtime() {
    let parent = tempfile::tempdir().unwrap();
    let root = parent.path().canonicalize().unwrap().join("alice");
    let cli = Arc::new(ScriptedCli::default());
    cli.ok("identity.show", serde_json::json!({"identity_pubkey": "a"}));
    let (releasing, reconcile) = gated();
    let client = CliClient::new(config(), cli)
        .with_storage(ActorStorage::Fresh(root.clone()))
        .reconciling_with(reconcile);
    client.start_fresh().unwrap();
    let _left_behind = ActiveRuntime::begin(&root).unwrap();

    let (invoked, ()) = tokio::join!(client.invoke(Call::local(&["identity", "show"])), releasing);

    invoked.unwrap();
    ensure_stopped(&root).unwrap();
}

/// A caller that gives up on reopening leaves the reconciliation running:
/// nothing else changes the storage until it ends, and it opens after.
#[tokio::test(flavor = "current_thread")]
async fn a_caller_that_gives_up_on_reopening_leaves_the_client_busy_until_reconciled() {
    let (_parent, root) = interrupted(Platform::Cli);
    let (releasing, reconcile) = gated();
    let client = resumed(&root, reconcile);

    let gave_up = tokio::time::timeout(Duration::from_millis(20), client.ensure_open()).await;
    let busy = client.ensure_open().await;
    releasing.await;
    client.settle().await.unwrap();
    client.ensure_open().await.unwrap();

    assert!(gave_up.is_err(), "the caller should have timed out");
    assert!(matches!(busy, Err(CliError::Storage(_))), "{busy:?}");
    assert_eq!(client.root(), Some(root.clone()));
    ensure_stopped(&root).unwrap();
}
