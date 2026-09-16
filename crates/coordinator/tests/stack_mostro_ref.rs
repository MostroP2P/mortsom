//! Real Docker and GitHub: builds a known Mostro ref and brings a stack up
//! on it. Ignored by default; the first build compiles mostrod, and the
//! stack takes the host's fixed ports and the `mortsom-regtest` project, so
//! run it only where no other Mortsom stack is up:
//! `cargo test -p mortsom-coordinator --test stack_mostro_ref -- --ignored`.
//! `MORTSOM_IT_MOSTRO_REF` selects the ref (default `main`).

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::Path;

use mortsom_coordinator::mostro_source::Selector;
use mortsom_coordinator::stack::{self, DaemonProvenance, DaemonSource, Layout};
use mortsom_ui_driver::process::TokioRunner;

const STACK_FILES: [&str; 4] = [
    "compose.yml",
    "limits.toml",
    "mostro-settings.tpl.toml",
    "relay-config.toml",
];

#[tokio::test]
#[ignore = "requires Docker, network access to GitHub and a host without another Mortsom stack"]
async fn a_mostro_ref_is_built_at_its_exact_commit_and_reaches_a_ready_stack() {
    // Arrange
    let requested = std::env::var("MORTSOM_IT_MOSTRO_REF").unwrap_or_else(|_| "main".into());
    let selector = Selector::parse(&requested).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let committed = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../stack");
    for name in STACK_FILES {
        std::fs::copy(committed.join(name), dir.path().join(name)).unwrap();
    }
    let layout = Layout::new(dir.path()).unwrap();
    let options = stack::UpOptions {
        daemon: DaemonSource::Ref(selector.clone()),
        source_cache: dir.path().join("source-cache"),
        timing: stack::Timing::default(),
        replace: false,
    };

    // Act
    let outcome = stack::up(&TokioRunner, &stack::LiveRelay, &layout, &options).await;
    // Observed before the teardown and asserted after it: a failed
    // expectation must not leave the stack running on the host.
    let ready = stack::status(&TokioRunner, &layout)
        .await
        .map(|status| status.is_ready());
    stack::down(&TokioRunner, &layout).await.unwrap();

    // Assert
    let descriptor = outcome.expect("stack up should build the ref and become ready");
    assert!(ready.unwrap(), "stack status should report ready");
    let DaemonProvenance::GitRef(selection) = &descriptor.daemon_source else {
        panic!("unexpected provenance {:?}", descriptor.daemon_source);
    };
    assert_eq!(selection.git_ref, selector.git_ref());
    assert_eq!(selection.revision.len(), 40);
    assert_eq!(
        descriptor.daemon_image.reference,
        stack::commit_tag(&selection.revision)
    );
    assert_eq!(descriptor.daemon_image.repository_digest, None);
}
