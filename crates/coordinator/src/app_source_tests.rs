#![allow(clippy::unwrap_used)]
use super::*;
use mortsom_ui_driver::process::TokioRunner;

#[test]
fn branch_and_pull_request_select_only_the_requested_head() {
    assert_eq!(
        Selector::parse("feature/foo").unwrap().git_ref(),
        "refs/heads/feature/foo"
    );
    for url in [
        "https://github.com/MostroP2P/app/pull/412",
        "https://github.com/MostroP2P/app/pull/412/",
    ] {
        assert_eq!(
            Selector::parse(url).unwrap().git_ref(),
            "refs/pull/412/head"
        );
    }
    for invalid in [
        "",
        "HEAD",
        "a.lock/b",
        "--upload-pack=evil",
        "foo..bar",
        "foo.lock",
        "a@{b}",
        "a b",
        "a\\b",
        "foo//bar",
        "refs/heads/main",
        "https://github.com/other/app/pull/1",
        "https://github.com/MostroP2P/app/pull/0",
        "https://github.com/MostroP2P/app/pull/1/files",
        "https://github.com/MostroP2P/app/pull/1?x=y",
    ] {
        assert!(Selector::parse(invalid).is_err(), "{invalid}");
    }
}

async fn git(dir: &Path, args: &[&str]) -> String {
    run_git(&TokioRunner, dir, args).await.unwrap()
}

#[tokio::test]
async fn moving_remote_heads_are_refetched_while_prepared_sources_stay_fixed() {
    let root = tempfile::tempdir().unwrap();
    let remote = root.path().join("remote");
    std::fs::create_dir(&remote).unwrap();
    git(&remote, &["init", "-b", "main"]).await;
    git(
        &remote,
        &[
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.org",
            "commit",
            "--allow-empty",
            "-m",
            "first",
        ],
    )
    .await;
    let first = git(&remote, &["rev-parse", "HEAD"]).await;
    // A fork PR head can be unrelated to the base branch.
    git(&remote, &["checkout", "--orphan", "fork"]).await;
    git(
        &remote,
        &[
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.org",
            "commit",
            "--allow-empty",
            "-m",
            "fork",
        ],
    )
    .await;
    let fork = git(&remote, &["rev-parse", "HEAD"]).await;
    git(&remote, &["update-ref", "refs/pull/42/head", fork.trim()]).await;
    let cache = root.path().join("managed");
    let selector = Selector::parse("main").unwrap();
    let redirected = RedirectedGit {
        foreign: remote.clone(),
    };
    let prepared = prepare_from(&cache, remote.to_str().unwrap(), &selector, &redirected)
        .await
        .unwrap();
    assert_eq!(prepared.selection.revision, first.trim());
    assert_eq!(
        git(&remote, &["rev-parse", "HEAD"]).await.trim(),
        fork.trim()
    );
    assert_eq!(
        git(&prepared.path, &["rev-parse", "HEAD"]).await.trim(),
        first.trim()
    );
    git(&remote, &["update-ref", "refs/heads/main", fork.trim()]).await;
    let newer = prepare_from(&cache, remote.to_str().unwrap(), &selector, &TokioRunner)
        .await
        .unwrap();
    assert_eq!(newer.selection.revision, fork.trim());
    assert_eq!(
        git(&prepared.path, &["rev-parse", "HEAD"]).await.trim(),
        first.trim()
    );
    let pr = Selector::parse("https://github.com/MostroP2P/app/pull/42").unwrap();
    let proposed = prepare_from(&cache, remote.to_str().unwrap(), &pr, &TokioRunner)
        .await
        .unwrap();
    assert_eq!(proposed.selection.revision, fork.trim());
    git(&remote, &["update-ref", "-d", "refs/heads/main"]).await;
    assert!(
        prepare_from(&cache, remote.to_str().unwrap(), &selector, &TokioRunner)
            .await
            .is_err(),
        "a missing remote ref must never reuse a cached head"
    );
    let path = prepared.path.clone();
    drop(prepared);
    assert!(!path.exists());
    assert!(cache.join("repository.git").exists());
}

#[tokio::test]
async fn fetch_errors_are_redacted_and_never_continue_to_a_checkout() {
    use mortsom_ui_driver::process::tests_support::FakeRunner;
    let root = tempfile::tempdir().unwrap();
    let runner = FakeRunner::default();
    runner.script("env", "true");
    runner.script(
        &git_command(
            &root.path().join("repository.git"),
            &["rev-parse", "--absolute-git-dir"],
        )
        .display(),
        &root.path().join("repository.git").display().to_string(),
    );
    runner.script_failure(
        &git_command(&root.path().join("repository.git"), &["fetch"]).display(),
        "network failed secret=private-fixture",
    );
    let error = prepare_from(
        root.path(),
        REPOSITORY,
        &Selector::parse("main").unwrap(),
        &runner,
    )
    .await
    .err()
    .unwrap()
    .to_string();
    assert!(error.contains("no cached fallback"));
    assert!(!error.contains("private-fixture"));
    assert!(!runner.calls().iter().any(|call| call.contains("clone")));
}

#[tokio::test]
async fn corrupt_fetched_identity_and_busy_cache_fail_closed() {
    use mortsom_ui_driver::process::tests_support::FakeRunner;
    let root = tempfile::tempdir().unwrap();
    let runner = FakeRunner::default();
    let selector = Selector::parse("main").unwrap();
    runner.script("env", "true");
    runner.script(
        &git_command(
            &root.path().join("repository.git"),
            &["rev-parse", "--absolute-git-dir"],
        )
        .display(),
        &root.path().join("repository.git").display().to_string(),
    );
    let error = prepare_from(root.path(), REPOSITORY, &selector, &runner)
        .await
        .err()
        .unwrap();
    assert!(matches!(error, SourceError::Verification(_)));
    let _lock = crate::lock::RunLock::acquire(root.path()).unwrap();
    let error = prepare_from(root.path(), REPOSITORY, &selector, &runner)
        .await
        .err()
        .unwrap();
    assert!(matches!(error, SourceError::Lock(_)));
}

#[tokio::test]
async fn foreign_cache_is_not_adopted_or_changed() {
    let root = tempfile::tempdir().unwrap();
    let cache = root.path().join("repository.git");
    std::fs::create_dir(&cache).unwrap();
    git(&cache, &["init", "-b", "main"]).await;
    let before = std::fs::read(cache.join(".git/config")).unwrap();
    let error = prepare_from(
        root.path(),
        REPOSITORY,
        &Selector::parse("main").unwrap(),
        &TokioRunner,
    )
    .await
    .err()
    .unwrap();
    assert!(matches!(error, SourceError::Ownership(_)));
    assert_eq!(std::fs::read(cache.join(".git/config")).unwrap(), before);
    assert!(!cache.join("mortsom-owner").exists());
}

#[cfg(unix)]
#[tokio::test]
async fn symlinked_cache_and_root_never_modify_another_checkout() {
    let root = tempfile::tempdir().unwrap();
    let foreign = root.path().join("foreign");
    std::fs::create_dir(&foreign).unwrap();
    git(&foreign, &["init", "-b", "main"]).await;
    let before = std::fs::read(foreign.join(".git/config")).unwrap();
    let cache = root.path().join("managed");
    std::fs::create_dir(&cache).unwrap();
    std::os::unix::fs::symlink(foreign.join(".git"), cache.join("repository.git")).unwrap();
    let alias = root.path().join("alias");
    std::os::unix::fs::symlink(&foreign, &alias).unwrap();
    for directory in [&cache, &alias] {
        let error = prepare_from(
            directory,
            REPOSITORY,
            &Selector::parse("main").unwrap(),
            &TokioRunner,
        )
        .await
        .err()
        .unwrap();
        assert!(matches!(error, SourceError::Ownership(_)));
    }
    assert_eq!(std::fs::read(foreign.join(".git/config")).unwrap(), before);
}

// A caller may launch Mortsom from a Git hook with repository variables set.
struct RedirectedGit {
    foreign: PathBuf,
}

#[async_trait::async_trait]
impl mortsom_ui_driver::process::CommandRunner for RedirectedGit {
    async fn run(
        &self,
        command: &Command,
        timeout: Duration,
    ) -> Result<mortsom_ui_driver::process::Output, mortsom_ui_driver::process::ProcessError> {
        let redirected = command
            .clone()
            .env("GIT_DIR", &self.foreign.join(".git").display().to_string())
            .env("GIT_WORK_TREE", &self.foreign.display().to_string());
        TokioRunner.run(&redirected, timeout).await
    }
    async fn spawn(
        &self,
        command: &Command,
        log: Option<&Path>,
    ) -> Result<
        Box<dyn mortsom_ui_driver::process::ChildProcess>,
        mortsom_ui_driver::process::ProcessError,
    > {
        TokioRunner.spawn(command, log).await
    }
}

#[tokio::test]
async fn managed_files_are_ignored_even_outside_the_mortsom_checkout() {
    let root = tempfile::tempdir().unwrap();
    git(root.path(), &["init", "-b", "main"]).await;
    let managed = root.path().join(".mortsom/app");
    std::fs::create_dir_all(&managed).unwrap();
    ignore_managed_contents(&managed).unwrap();
    std::fs::write(managed.join("app-source"), "managed contents").unwrap();
    assert_eq!(
        git(root.path(), &["check-ignore", ".mortsom/app/app-source"])
            .await
            .trim(),
        ".mortsom/app/app-source"
    );
    assert!(
        git(root.path(), &["status", "--porcelain"])
            .await
            .is_empty()
    );
}
