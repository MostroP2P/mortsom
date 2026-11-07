//! Effective source configuration against a fake ready stack.
use super::*;
use crate::{config::AppSource, desktop, doctor::RealHost};

fn selections() -> [AppSource; 3] {
    [
        AppSource::Remote(crate::app_source::Selector::parse("feature/fix").unwrap()),
        AppSource::Local("/developer/app".into()),
        AppSource::Local("env:literal-directory".into()),
    ]
}

async fn ready_config() -> (tempfile::TempDir, ScriptedStack, PathBuf) {
    let (dir, layout) = scratch();
    let docker = ScriptedStack::fresh(&layout);
    up(&docker, &Publishes(true), &layout, &quick())
        .await
        .unwrap();
    for actor in ["alice", "bob"] {
        let credentials = dir.path().join("local/creds").join(actor);
        std::fs::create_dir_all(&credentials).unwrap();
        for name in ["tls.cert", "admin.macaroon"] {
            std::fs::write(credentials.join(name), b"fixture").unwrap();
        }
    }
    let path = dir.path().join("mortsom.toml");
    std::fs::write(
        &path,
        include_str!("../../../../config/desktop.example.toml"),
    )
    .unwrap();
    desktop::load(&path, dir.path(), &docker, &RealHost)
        .await
        .unwrap();
    (dir, docker, path)
}

fn assert_selection(config: &crate::Config, selection: &AppSource) {
    match selection {
        AppSource::Remote(reference) => {
            assert!(config.mobile.source_dir.is_empty());
            assert_eq!(
                config.mobile.app_ref.as_deref(),
                Some(reference.to_string().as_str())
            );
        }
        AppSource::Local(path) => {
            assert_eq!(config.mobile.source_dir, path.to_str().unwrap());
            assert!(config.mobile.app_ref.is_none());
        }
    }
}

#[tokio::test]
async fn run_configuration_replaces_invalid_source_modes_before_loading() {
    let (dir, docker, path) = ready_config().await;
    for configured in [
        "source_dir = '/stale/app'\napp_ref = 'main'",
        "app_ref = 'https://github.com/other/app/pull/1'",
        "source_dir = 'env:UNUSED_SOURCE'\napp_ref = 'env:UNUSED_REF'",
        "source_dir = 42\napp_ref = false",
    ] {
        std::fs::write(
            dir.path().join("mortsom.local.toml"),
            format!("[mobile]\n{configured}\n"),
        )
        .unwrap();
        assert!(
            desktop::load(&path, dir.path(), &docker, &RealHost)
                .await
                .is_err()
        );
        for selection in selections() {
            let (config, _) =
                desktop::load_with_source(&path, dir.path(), &docker, &RealHost, Some(&selection))
                    .await
                    .unwrap();
            assert_selection(&config, &selection);
        }
    }
}

#[tokio::test]
async fn source_overrides_preserve_caps_credentials_and_other_validation() {
    let (dir, docker, path) = ready_config().await;
    let context = StackContext::resolve(dir.path(), &docker).await.unwrap();
    for invalid in [
        "[run]\nmax_order_sats = 0",
        "[run]\nmax_run_spend_sats = 0",
        "[run]\namount_sats = 0",
        "[wallets.alice]\nmacaroon = '/literal/credential'",
        "[wallets.alice]\nmacaroon = 'env:MISSING_REQUIRED_CREDENTIAL'",
        "[mobile]\nprofile = 'invalid-profile'",
        "[mobile]\nunknown_field = true",
    ] {
        std::fs::write(dir.path().join("mortsom.local.toml"), invalid).unwrap();
        for selection in selections() {
            assert!(
                desktop::load_with_source(&path, dir.path(), &docker, &RealHost, Some(&selection),)
                    .await
                    .is_err(),
                "{invalid}"
            );
            let diagnosis = crate::Config::diagnose_with_source(
                &path,
                |name| context.lookup(name),
                Some(&selection),
            );
            assert!(
                !diagnosis.problems.is_empty() || !diagnosis.missing_env.is_empty(),
                "{invalid}"
            );
        }
    }
}

/// Until `run` can drive the `cli` profile, a configuration naming it is
/// refused at load, before anything is admitted, built or started.
#[tokio::test]
async fn run_and_recovery_refuse_a_cli_configuration() {
    let (dir, docker, path) = ready_config().await;
    std::fs::write(
        dir.path().join("mortsom.local.toml"),
        "[mobile]\nprofile = 'cli'\n",
    )
    .unwrap();
    let refusal = "Linux and Web require mobile.profile = mobile-v2";

    let Err(error) = desktop::load(&path, dir.path(), &docker, &RealHost).await else {
        panic!("a cli configuration loaded");
    };
    assert!(error.contains(refusal), "{error}");
    for selection in selections() {
        let loaded =
            desktop::load_with_source(&path, dir.path(), &docker, &RealHost, Some(&selection))
                .await;
        let Err(error) = loaded else {
            panic!("a cli configuration loaded with {selection:?}");
        };
        assert!(error.contains(refusal), "{error}");
    }
}

#[test]
fn credential_references_live_under_the_stack_directory() {
    // Act / Assert
    assert_eq!(
        credential_reference(Path::new("/s"), "MORTSOM_ALICE_LND_CERT").unwrap(),
        Path::new("/s/local/creds/alice/tls.cert")
    );
    assert_eq!(
        credential_reference(Path::new("/s"), "MORTSOM_BOB_LND_MACAROON").unwrap(),
        Path::new("/s/local/creds/bob/admin.macaroon")
    );
    assert!(credential_reference(Path::new("/s"), "MORTSOM_CAROL_LND_CERT").is_none());
    assert!(credential_reference(Path::new("/s"), "MORTSOM_MOSTRO_PUBKEY").is_none());
}
