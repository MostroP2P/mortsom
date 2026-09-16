//! `stack up --mostro-ref` with real Git against a local remote standing in
//! for the Mostro repository, and a scripted Docker.
use super::*;
use crate::mostro_source::{self, Selector};
use mortsom_ui_driver::process::TokioRunner;

const PR: &str = "https://github.com/MostroP2P/mostro/pull/963";
const REVISION_LABEL: &str = "org.opencontainers.image.revision";

/// The image ID the scripted build gives a commit: the same commit always
/// builds into the same image, as it does with a warm layer cache.
fn image_id(revision: &str) -> String {
    format!("sha256:{revision}{}", "0".repeat(24))
}

async fn git(dir: &Path, args: &[&str]) -> String {
    let mut argv = vec![
        "-C",
        dir.to_str().unwrap(),
        "-c",
        "user.name=Test",
        "-c",
        "user.email=test@example.org",
    ];
    argv.extend_from_slice(args);
    mortsom_ui_driver::process::run_ok(
        &TokioRunner,
        &Command::new("git", &argv),
        Duration::from_secs(30),
    )
    .await
    .unwrap()
    .stdout
    .trim()
    .to_owned()
}

/// Commits a buildable tree and returns the commit.
async fn commit(remote: &Path, message: &str) -> String {
    std::fs::create_dir_all(remote.join("docker")).unwrap();
    std::fs::write(remote.join("docker/Dockerfile"), "FROM scratch\n").unwrap();
    std::fs::write(remote.join("version"), message).unwrap();
    git(remote, &["add", "-A"]).await;
    git(remote, &["commit", "-m", message]).await;
    git(remote, &["rev-parse", "HEAD"]).await
}

/// A remote whose `main` holds one commit.
async fn remote() -> (tempfile::TempDir, PathBuf, String) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("mostro");
    std::fs::create_dir(&path).unwrap();
    git(&path, &["init", "-b", "main"]).await;
    let main = commit(&path, "main").await;
    (dir, path, main)
}

/// Docker as `ScriptedStack` plays it, plus source builds. Git runs for
/// real, with the upstream URL pointed at the local remote.
struct RefStack {
    docker: ScriptedStack,
    remote: PathBuf,
    build_fails: bool,
    built: Mutex<Vec<String>>,
    contexts_with_git: Mutex<usize>,
    started: Mutex<Option<String>>,
}

impl RefStack {
    fn new(layout: &Layout, remote: &Path) -> Self {
        Self {
            docker: ScriptedStack::fresh(layout),
            remote: remote.to_path_buf(),
            build_fails: false,
            built: Mutex::new(Vec::new()),
            contexts_with_git: Mutex::new(0),
            started: Mutex::new(None),
        }
    }

    fn calls(&self) -> Vec<String> {
        self.docker.calls()
    }

    fn record(&self, key: String) {
        self.docker.calls.lock().unwrap().push(key);
    }

    fn built_revision(tag: &str) -> Option<&str> {
        tag.strip_prefix(LOCAL_REPOSITORY)?.strip_prefix(":git-")
    }

    fn build(&self, key: &str, args: &[String]) -> Output {
        let context = PathBuf::from(args.last().unwrap());
        if context.join(".git").exists() {
            *self.contexts_with_git.lock().unwrap() += 1;
        }
        if self.build_fails {
            return failure("error: could not compile `mostro`");
        }
        let tag = &args[args.iter().position(|arg| arg == "--tag").unwrap() + 1];
        self.built.lock().unwrap().push(tag.clone());
        assert!(key.contains(&format!("{REVISION_LABEL}=")), "{key}");
        success(String::new())
    }

    fn inspect_built(&self, tag: &str) -> Output {
        let Some(revision) = Self::built_revision(tag) else {
            return failure("not a source build");
        };
        if !self.built.lock().unwrap().iter().any(|built| built == tag) {
            return failure("No such image");
        }
        success(
            serde_json::json!({
                "id": image_id(revision),
                "labels": {REVISION_LABEL: revision},
            })
            .to_string(),
        )
    }

    /// What `docker inspect` says about the running daemon, once it was
    /// started from a source build.
    fn running_build(&self) -> Option<Output> {
        let started = self.started.lock().unwrap().clone()?;
        let revision = Self::built_revision(&started)?;
        let reply = serde_json::json!({"image": image_id(revision), "running": true});
        Some(success(reply.to_string()))
    }
}

fn success(stdout: String) -> Output {
    Output {
        success: true,
        stdout,
        stderr: String::new(),
    }
}

fn failure(stderr: &str) -> Output {
    Output {
        success: false,
        stdout: String::new(),
        stderr: stderr.to_owned(),
    }
}

#[async_trait]
impl CommandRunner for RefStack {
    async fn run(&self, command: &Command, timeout: Duration) -> Result<Output, ProcessError> {
        if command.program == Path::new("env") {
            let mut redirected = command.clone();
            for arg in &mut redirected.args {
                if arg == mostro_source::REPOSITORY {
                    *arg = self.remote.display().to_string();
                }
            }
            self.record(format!("git {}", command.args.join(" ")));
            return TokioRunner.run(&redirected, timeout).await;
        }
        let key = format!("{} {}", command.program.display(), command.args.join(" "));
        let last = command.args.last().map(String::as_str).unwrap_or_default();
        if key.starts_with("docker build") {
            self.record(key.clone());
            return Ok(self.build(&key, &command.args));
        }
        if key.starts_with("docker image inspect") && Self::built_revision(last).is_some() {
            self.record(key);
            return Ok(self.inspect_built(last));
        }
        if key.contains("up -d --force-recreate mostro") {
            *self.started.lock().unwrap() = command.env.get("MORTSOM_MOSTRO_IMAGE").cloned();
        }
        let container_image = key.starts_with("docker inspect") && !key.contains("working_dir");
        if container_image && let Some(reply) = self.running_build() {
            self.record(key);
            return Ok(reply);
        }
        self.docker.run(command, timeout).await
    }

    async fn spawn(
        &self,
        _command: &Command,
        _log_file: Option<&Path>,
    ) -> Result<Box<dyn ChildProcess>, ProcessError> {
        unreachable!("the stack never spawns")
    }
}

fn from_ref(value: &str, cache: &Path) -> UpOptions {
    UpOptions {
        daemon: DaemonSource::Ref(Selector::parse(value).unwrap()),
        source_cache: cache.to_path_buf(),
        ..quick()
    }
}

fn leftover_checkouts(cache: &Path) -> Vec<String> {
    std::fs::read_dir(cache)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|name| name.starts_with("build-"))
        .collect()
}

#[tokio::test]
async fn up_builds_and_runs_the_head_commit_of_a_fork_pull_request() {
    // Arrange: the fork's head shares no history with the base branch.
    let (_remote_dir, remote, _main) = remote().await;
    git(&remote, &["checkout", "--orphan", "fork"]).await;
    let head = commit(&remote, "fork change").await;
    git(&remote, &["update-ref", "refs/pull/963/head", &head]).await;
    git(&remote, &["checkout", "main"]).await;
    let (_dir, layout) = scratch();
    let cache = layout.root().join("source-cache");
    let docker = RefStack::new(&layout, &remote);

    // Act
    let descriptor = up(&docker, &Publishes(true), &layout, &from_ref(PR, &cache))
        .await
        .unwrap();

    // Assert
    let DaemonProvenance::GitRef(selection) = &descriptor.daemon_source else {
        panic!("{:?}", descriptor.daemon_source);
    };
    assert_eq!(selection.revision, head);
    assert_eq!(selection.requested, PR);
    assert_eq!(selection.git_ref, "refs/pull/963/head");
    assert_eq!(selection.repository, mostro_source::REPOSITORY);
    assert_eq!(descriptor.daemon_image.reference, commit_tag(&head));
    assert_eq!(descriptor.daemon_image.image_id, image_id(&head));
    assert_eq!(descriptor.daemon_image.repository_digest, None);
    assert_eq!(
        docker.started.lock().unwrap().as_deref(),
        Some(commit_tag(&head).as_str())
    );
    assert!(
        docker
            .calls()
            .iter()
            .all(|call| !call.contains("docker pull"))
    );
    assert_eq!(*docker.contexts_with_git.lock().unwrap(), 0);
    assert!(leftover_checkouts(&cache).is_empty());
    let recorded = Descriptor::read(&layout).unwrap().unwrap();
    assert_eq!(recorded, descriptor);
    assert!(status(&docker, &layout).await.unwrap().is_ready());
}

#[tokio::test]
async fn up_fetches_the_branch_again_and_recreates_the_daemon_on_its_new_commit() {
    // Arrange
    let (_remote_dir, remote, first) = remote().await;
    let (_dir, layout) = scratch();
    let cache = layout.root().join("source-cache");
    let docker = RefStack::new(&layout, &remote);
    let options = from_ref("main", &cache);
    let before = up(&docker, &Publishes(true), &layout, &options)
        .await
        .unwrap();
    let second = commit(&remote, "moved").await;

    // Act
    let after = up(&docker, &Publishes(true), &layout, &options)
        .await
        .unwrap();

    // Assert
    let fetches = docker
        .calls()
        .iter()
        .filter(|call| call.contains(" fetch "))
        .count();
    assert_eq!(fetches, 2);
    assert_eq!(before.daemon_image.reference, commit_tag(&first));
    assert_eq!(after.daemon_image.reference, commit_tag(&second));
    assert_ne!(before.daemon_pubkey, after.daemon_pubkey);
    assert_ne!(before.fingerprint, after.fingerprint);
    assert_eq!(
        docker.started.lock().unwrap().as_deref(),
        Some(commit_tag(&second).as_str())
    );
}

#[tokio::test]
async fn the_same_commit_builds_again_into_the_same_image() {
    // Arrange
    let (_remote_dir, remote, head) = remote().await;
    let (_dir, layout) = scratch();
    let cache = layout.root().join("source-cache");
    let docker = RefStack::new(&layout, &remote);
    let options = from_ref("main", &cache);

    // Act
    let first = up(&docker, &Publishes(true), &layout, &options)
        .await
        .unwrap();
    let second = up(&docker, &Publishes(true), &layout, &options)
        .await
        .unwrap();

    // Assert
    assert_eq!(first.daemon_image, second.daemon_image);
    assert_eq!(first.daemon_source, second.daemon_source);
    assert_eq!(first.daemon_image.image_id, image_id(&head));
    assert_eq!(docker.built.lock().unwrap().len(), 2);
    assert_eq!(*docker.contexts_with_git.lock().unwrap(), 0);
}

#[tokio::test]
async fn a_failed_build_never_starts_the_image_an_earlier_commit_left_behind() {
    // Arrange
    let (_remote_dir, remote, _first) = remote().await;
    let (_dir, layout) = scratch();
    let cache = layout.root().join("source-cache");
    let mut docker = RefStack::new(&layout, &remote);
    let options = from_ref("main", &cache);
    up(&docker, &Publishes(true), &layout, &options)
        .await
        .unwrap();
    commit(&remote, "does not compile").await;
    docker.build_fails = true;
    let calls_before = docker.calls().len();

    // Act
    let error = up(&docker, &Publishes(true), &layout, &options)
        .await
        .unwrap_err();

    // Assert
    assert!(error.to_string().contains("could not compile"), "{error}");
    let later = &docker.calls()[calls_before..];
    assert!(
        later.iter().all(|call| !call.contains("compose up")),
        "{later:?}"
    );
    assert!(leftover_checkouts(&cache).is_empty());
}

#[tokio::test]
async fn a_missing_ref_fails_before_anything_is_built_or_started() {
    // Arrange
    let (_remote_dir, remote, _main) = remote().await;
    let (_dir, layout) = scratch();
    let cache = layout.root().join("source-cache");
    let docker = RefStack::new(&layout, &remote);
    up(
        &docker,
        &Publishes(true),
        &layout,
        &from_ref("main", &cache),
    )
    .await
    .unwrap();
    let calls_before = docker.calls().len();

    // Act: an earlier fetch left objects behind, but this branch never existed.
    let error = up(
        &docker,
        &Publishes(true),
        &layout,
        &from_ref("gone", &cache),
    )
    .await
    .unwrap_err();

    // Assert
    assert!(matches!(error, StackError::Source(_)), "{error}");
    assert!(error.to_string().contains("no cached fallback"), "{error}");
    let later = &docker.calls()[calls_before..];
    assert!(
        later
            .iter()
            .all(|call| !call.contains("docker build") && !call.contains("compose up")),
        "{later:?}"
    );
}

#[tokio::test]
async fn a_commit_without_the_supported_dockerfile_is_refused_without_building() {
    // Arrange
    let (_remote_dir, remote, _main) = remote().await;
    git(&remote, &["rm", "-r", "-q", "docker"]).await;
    git(&remote, &["commit", "-m", "no docker"]).await;
    let (_dir, layout) = scratch();
    let cache = layout.root().join("source-cache");
    let docker = RefStack::new(&layout, &remote);

    // Act
    let error = up(
        &docker,
        &Publishes(true),
        &layout,
        &from_ref("main", &cache),
    )
    .await
    .unwrap_err();

    // Assert
    assert!(error.to_string().contains("docker/Dockerfile"), "{error}");
    assert!(docker.built.lock().unwrap().is_empty());
    assert!(leftover_checkouts(&cache).is_empty());
}

#[tokio::test]
async fn a_selected_image_is_still_pulled_and_recorded_as_requested() {
    // Arrange
    let (_dir, layout) = scratch();
    let docker = ScriptedStack::fresh(&layout);
    let options = UpOptions {
        daemon: DaemonSource::Image("mostrop2p/mostro:v0.18.8".into()),
        ..quick()
    };

    // Act
    let descriptor = up(&docker, &Publishes(true), &layout, &options)
        .await
        .unwrap();

    // Assert
    assert_eq!(
        docker
            .matching("docker pull mostrop2p/mostro:v0.18.8")
            .len(),
        1
    );
    assert_eq!(
        descriptor.daemon_source,
        DaemonProvenance::Image {
            requested: "mostrop2p/mostro:v0.18.8".into()
        }
    );
    assert!(docker.matching("docker build").is_empty());
}

#[tokio::test]
async fn no_selection_pulls_the_pinned_release() {
    // Arrange
    let (_dir, layout) = scratch();
    let docker = ScriptedStack::fresh(&layout);

    // Act
    let descriptor = up(&docker, &Publishes(true), &layout, &quick())
        .await
        .unwrap();

    // Assert
    let pulls = docker.matching(&format!("docker pull {DEFAULT_MOSTRO_IMAGE}"));
    assert_eq!(pulls.len(), 1);
    assert_eq!(descriptor.daemon_source, DaemonProvenance::PinnedImage);
}
