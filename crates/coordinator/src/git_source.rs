//! Managed upstream checkouts: fetch a head once, then prepare an isolated
//! exact revision. The app and the daemon each name their own upstream; the
//! guarantees are the same for both.
use mortsom_ui_driver::process::{Command, CommandRunner, run_ok};
use serde::{Deserialize, Serialize};
use std::{
    marker::PhantomData,
    path::{Path, PathBuf},
    time::Duration,
};

/// One upstream repository remote selectors may name, and where its managed
/// cache lives. Nothing else is ever fetched.
pub trait Upstream: Send + Sync + 'static {
    /// Component name used in messages and the cache ownership marker.
    const COMPONENT: &'static str;
    /// The only repository accepted by this upstream's selectors.
    const REPOSITORY: &'static str;
    /// Canonical pull-request URL prefix, ending in `/pull/`.
    const PULL_PREFIX: &'static str;
    /// Ignored workspace directory; source never enters Mortsom Git.
    const WORK_DIR: &'static str;
}

/// Validated branch name or pull-request URL of upstream `U`.
pub struct Selector<U> {
    requested: String,
    git_ref: String,
    upstream: PhantomData<U>,
}

impl<U> Clone for Selector<U> {
    fn clone(&self) -> Self {
        Self {
            requested: self.requested.clone(),
            git_ref: self.git_ref.clone(),
            upstream: PhantomData,
        }
    }
}

impl<U> PartialEq for Selector<U> {
    fn eq(&self, other: &Self) -> bool {
        self.requested == other.requested && self.git_ref == other.git_ref
    }
}

impl<U> Eq for Selector<U> {}

impl<U> std::fmt::Debug for Selector<U> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Selector")
            .field("requested", &self.requested)
            .field("git_ref", &self.git_ref)
            .finish()
    }
}

/// Failures preparing managed source, before anything is built or started.
#[derive(Debug, thiserror::Error)]
pub enum SourceError {
    /// Unsupported or ambiguous selector; holds the component and its
    /// pull-request URL prefix.
    #[error("invalid {0} selector: use a branch name (e.g. main) or {1}123")]
    InvalidSelector(&'static str, &'static str),
    /// Local working-directory failure.
    #[error("cannot prepare managed source directory: {0}")]
    Io(#[from] std::io::Error),
    /// A path was not created by this managed-source implementation.
    #[error(
        "refusing unowned managed source cache at {0}; choose an empty directory or inspect and move the conflicting path"
    )]
    Ownership(PathBuf),
    /// Bounded Git operation failed; holds the component and the redacted cause.
    #[error(
        "{0} Git operation failed; verify Git is installed, network access and the branch/PR exists (no cached fallback): {1}"
    )]
    Git(&'static str, String),
    /// Corrupt or unexpected Git state.
    #[error("managed source revision verification failed: {0}")]
    Verification(String),
    /// Concurrent source preparation.
    #[error(transparent)]
    Lock(#[from] crate::lock::LockError),
}

impl<U: Upstream> Selector<U> {
    /// Parses a plain branch or the upstream's PR URL, never a merge ref.
    pub fn parse(value: &str) -> Result<Self, SourceError> {
        let invalid = || SourceError::InvalidSelector(U::COMPONENT, U::PULL_PREFIX);
        let git_ref = if let Some(number) = value.strip_prefix(U::PULL_PREFIX) {
            let number = number.strip_suffix('/').unwrap_or(number);
            if number.is_empty()
                || !number.bytes().all(|b| b.is_ascii_digit())
                || number.starts_with('0')
                || number.parse::<u64>().is_err()
            {
                return Err(invalid());
            }
            format!("refs/pull/{number}/head")
        } else if valid_branch(value) {
            format!("refs/heads/{value}")
        } else {
            return Err(invalid());
        };
        Ok(Self {
            requested: value.to_owned(),
            git_ref,
            upstream: PhantomData,
        })
    }
}

impl<U> Selector<U> {
    /// The upstream's `main` branch: what is built when nothing is named.
    pub fn main() -> Self {
        Self {
            requested: "main".to_owned(),
            git_ref: "refs/heads/main".to_owned(),
            upstream: PhantomData,
        }
    }

    /// Exact remote ref to fetch.
    pub fn git_ref(&self) -> &str {
        &self.git_ref
    }

    /// The operator's branch or PR URL, as given.
    pub fn requested(&self) -> &str {
        &self.requested
    }
}

impl<U> std::fmt::Display for Selector<U> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.requested)
    }
}

impl<U: Upstream> std::str::FromStr for Selector<U> {
    type Err = SourceError;
    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::parse(value)
    }
}

fn valid_branch(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 240
        && value != "@"
        && value != "HEAD"
        && !value.starts_with('-')
        && !value.starts_with("refs/")
        && !value.ends_with('.')
        && !value.contains("..")
        && !value.contains("@{")
        && !value
            .bytes()
            .any(|b| b <= b' ' || b == 127 || b"~^:?*[\\".contains(&b))
        && value
            .split('/')
            .all(|part| !part.is_empty() && !part.starts_with('.') && !part.ends_with(".lock"))
}

/// Public remote-selection facts persisted in build and recovery provenance.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemoteSelection {
    /// Upstream repository URL.
    pub repository: String,
    /// Operator's branch or PR URL.
    pub requested: String,
    /// Fully qualified branch or PR head ref.
    pub git_ref: String,
    /// Full commit ID fetched before building.
    pub revision: String,
}

/// Private disposable build tree backed by reusable Git objects.
pub struct PreparedSource {
    /// Detached source checkout.
    pub path: PathBuf,
    /// Resolved public source identity.
    pub selection: RemoteSelection,
    _directory: tempfile::TempDir,
}

impl PreparedSource {
    /// Wraps an existing checkout; tests stand one up without Git.
    #[cfg(test)]
    pub(crate) fn new(
        path: PathBuf,
        selection: RemoteSelection,
        directory: tempfile::TempDir,
    ) -> Self {
        Self {
            path,
            selection,
            _directory: directory,
        }
    }
}

/// Fetches the requested head, including GitHub's head ref for fork PRs.
pub async fn prepare<U: Upstream>(
    selector: &Selector<U>,
    runner: &dyn CommandRunner,
) -> Result<PreparedSource, SourceError> {
    prepare_from(Path::new(U::WORK_DIR), U::REPOSITORY, selector, runner).await
}

pub(crate) async fn prepare_from<U: Upstream>(
    root: &Path,
    repository: &str,
    selector: &Selector<U>,
    runner: &dyn CommandRunner,
) -> Result<PreparedSource, SourceError> {
    reject_symlinks(root)?;
    std::fs::create_dir_all(root)?;
    let root = root.canonicalize()?;
    // The cache ref/FETCH_HEAD is mutable: serialize even callers outside the CLI.
    let _lock = crate::lock::RunLock::acquire(&root)?;
    ignore_managed_contents(&root)?;
    let cache = cache(&root, U::COMPONENT, runner).await?;
    let reference = format!("+{}:refs/mortsom/selected", selector.git_ref());
    run_git::<U>(
        runner,
        &cache,
        &["fetch", "--no-tags", "--force", repository, &reference],
    )
    .await?;
    let revision = run_git::<U>(
        runner,
        &cache,
        &["rev-parse", "--verify", "refs/mortsom/selected^{commit}"],
    )
    .await?;
    let revision = revision.trim();
    if revision.len() != 40 || !revision.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(SourceError::Verification(
            "Git did not return a full commit ID".into(),
        ));
    }
    checkout(&root, &cache, repository, selector, revision, runner).await
}

async fn checkout<U: Upstream>(
    root: &Path,
    cache: &Path,
    repository: &str,
    selector: &Selector<U>,
    revision: &str,
    runner: &dyn CommandRunner,
) -> Result<PreparedSource, SourceError> {
    let directory = tempfile::Builder::new().prefix("build-").tempdir_in(root)?;
    let path = directory.path().join("source");
    let cache = cache
        .to_str()
        .ok_or_else(|| SourceError::Verification("cache path must be UTF-8".into()))?;
    run_git::<U>(
        runner,
        directory.path(),
        &["clone", "--local", "--no-checkout", "--", cache, "source"],
    )
    .await?;
    run_git::<U>(runner, &path, &["checkout", "--detach", revision, "--"]).await?;
    if run_git::<U>(runner, &path, &["rev-parse", "HEAD"])
        .await?
        .trim()
        != revision
        || !run_git::<U>(
            runner,
            &path,
            &["status", "--porcelain", "--untracked-files=all"],
        )
        .await?
        .is_empty()
    {
        return Err(SourceError::Verification(
            "checkout does not match the fetched clean head".into(),
        ));
    }
    tracing::info!(
        revision,
        selector = %selector.requested,
        component = U::COMPONENT,
        "prepared managed source"
    );
    Ok(PreparedSource {
        path,
        selection: RemoteSelection {
            repository: repository.into(),
            requested: selector.requested.clone(),
            git_ref: selector.git_ref.clone(),
            revision: revision.into(),
        },
        _directory: directory,
    })
}

fn reject_symlinks(path: &Path) -> Result<(), SourceError> {
    let mut current = PathBuf::new();
    for part in path.components() {
        current.push(part);
        match std::fs::symlink_metadata(&current) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                return Err(SourceError::Ownership(current));
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }
    Ok(())
}

fn ignore_managed_contents(root: &Path) -> Result<(), SourceError> {
    use std::io::Write;
    let path = root.join(".gitignore");
    reject_symlinks(&path)?;
    match std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
    {
        Ok(mut file) => file.write_all(b"*\n")?,
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            if std::fs::read_to_string(&path)? != "*\n" {
                return Err(SourceError::Ownership(path));
            }
        }
        Err(error) => return Err(error.into()),
    }
    Ok(())
}

async fn cache(
    root: &Path,
    component: &'static str,
    runner: &dyn CommandRunner,
) -> Result<PathBuf, SourceError> {
    let cache = root.join("repository.git");
    reject_symlinks(&cache)?;
    // The app marker predates other upstreams; its spelling is unchanged so
    // existing caches stay adopted.
    let marker = format!("mortsom-managed-{component}-cache-v1\n");
    if !cache.exists() {
        let staging = tempfile::Builder::new().prefix("init-").tempdir_in(root)?;
        run_git_as(component, runner, staging.path(), &["init", "--bare"]).await?;
        std::fs::write(staging.path().join("mortsom-owner"), &marker)?;
        std::fs::rename(staging.path(), &cache)?;
    }
    let owner = cache.join("mortsom-owner");
    reject_symlinks(&owner)?;
    if std::fs::read_to_string(&owner).ok().as_deref() != Some(marker.as_str()) {
        return Err(SourceError::Ownership(cache));
    }
    if run_git_as(
        component,
        runner,
        &cache,
        &["rev-parse", "--is-bare-repository"],
    )
    .await?
    .trim()
        != "true"
    {
        return Err(SourceError::Ownership(cache));
    }
    let identity = run_git_as(
        component,
        runner,
        &cache,
        &["rev-parse", "--absolute-git-dir"],
    )
    .await?;
    if Path::new(identity.trim()) != cache {
        return Err(SourceError::Ownership(cache));
    }
    Ok(cache)
}

fn git_command(directory: &Path, args: &[&str]) -> Command {
    let mut command = Command::new("env", &[]);
    // A caller's Git environment must not redirect our commands into another checkout.
    for variable in [
        "GIT_DIR",
        "GIT_WORK_TREE",
        "GIT_COMMON_DIR",
        "GIT_INDEX_FILE",
        "GIT_OBJECT_DIRECTORY",
        "GIT_ALTERNATE_OBJECT_DIRECTORIES",
        "GIT_CONFIG",
        "GIT_CONFIG_COUNT",
        "GIT_CONFIG_PARAMETERS",
        "GIT_SHALLOW_FILE",
        "GIT_NAMESPACE",
    ] {
        command.args.extend(["-u".into(), variable.into()]);
    }
    command
        .args
        .extend(["git".into(), "-C".into(), directory.display().to_string()]);
    command
        .args
        .extend(args.iter().map(|arg| (*arg).to_owned()));
    command.env("GIT_TERMINAL_PROMPT", "0")
}

/// The Git command that fails unless `directory` tracks every one of `paths`.
pub(crate) fn tracks_command(directory: &Path, paths: &[&str]) -> Command {
    let mut args = vec!["ls-files", "--error-unmatch", "--"];
    args.extend_from_slice(paths);
    git_command(directory, &args)
}

async fn run_git<U: Upstream>(
    runner: &dyn CommandRunner,
    directory: &Path,
    args: &[&str],
) -> Result<String, SourceError> {
    run_git_as(U::COMPONENT, runner, directory, args).await
}

async fn run_git_as(
    component: &'static str,
    runner: &dyn CommandRunner,
    directory: &Path,
    args: &[&str],
) -> Result<String, SourceError> {
    run_ok(
        runner,
        &git_command(directory, args),
        Duration::from_secs(300),
    )
    .await
    .map(|output| output.stdout)
    .map_err(|error| {
        SourceError::Git(
            component,
            mortsom_common::Redactor::default().redact(&error.to_string()),
        )
    })
}

#[cfg(test)]
#[path = "git_source_tests.rs"]
mod tests;
