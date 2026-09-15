//! Managed app checkouts: fetch a head once, then build an isolated exact revision.
use mortsom_ui_driver::process::{Command, CommandRunner, run_ok};
use serde::{Deserialize, Serialize};
use std::{
    path::{Path, PathBuf},
    time::Duration,
};

/// The only repository accepted by remote selectors.
pub const REPOSITORY: &str = "https://github.com/MostroP2P/app.git";
/// Ignored workspace directory; source and build outputs never enter Mortsom Git.
pub const WORK_DIR: &str = ".mortsom/app";

/// Validated branch name or GitHub pull-request URL.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Selector {
    requested: String,
    git_ref: String,
}

/// Failures preparing managed source, before any client starts.
#[derive(Debug, thiserror::Error)]
pub enum SourceError {
    /// Unsupported or ambiguous selector.
    #[error(
        "invalid app selector: use a branch name (e.g. main) or https://github.com/MostroP2P/app/pull/123"
    )]
    InvalidSelector,
    /// Local working-directory failure.
    #[error("cannot prepare managed app directory: {0}")]
    Io(#[from] std::io::Error),
    /// A path was not created by this managed-source implementation.
    #[error(
        "refusing unowned managed app cache at {0}; choose an empty .mortsom/app directory or inspect and move the conflicting path"
    )]
    Ownership(PathBuf),
    /// Bounded Git operation failed.
    #[error(
        "app Git operation failed; verify Git is installed, network access and the branch/PR exists (no cached fallback): {0}"
    )]
    Git(String),
    /// Corrupt or unexpected Git state.
    #[error("managed app revision verification failed: {0}")]
    Verification(String),
    /// Concurrent source preparation.
    #[error(transparent)]
    Lock(#[from] crate::lock::LockError),
}

impl Selector {
    /// Parses a plain branch or the app repository's PR URL, never a merge ref.
    pub fn parse(value: &str) -> Result<Self, SourceError> {
        let git_ref =
            if let Some(number) = value.strip_prefix("https://github.com/MostroP2P/app/pull/") {
                let number = number.strip_suffix('/').unwrap_or(number);
                if number.is_empty()
                    || !number.bytes().all(|b| b.is_ascii_digit())
                    || number.starts_with('0')
                    || number.parse::<u64>().is_err()
                {
                    return Err(SourceError::InvalidSelector);
                }
                format!("refs/pull/{number}/head")
            } else if valid_branch(value) {
                format!("refs/heads/{value}")
            } else {
                return Err(SourceError::InvalidSelector);
            };
        Ok(Self {
            requested: value.to_owned(),
            git_ref,
        })
    }

    /// Exact remote ref to fetch.
    pub fn git_ref(&self) -> &str {
        &self.git_ref
    }
}

impl std::fmt::Display for Selector {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.requested)
    }
}

impl std::str::FromStr for Selector {
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

/// Fetches the requested app head, including GitHub's head ref for fork PRs.
pub async fn prepare(
    selector: &Selector,
    runner: &dyn CommandRunner,
) -> Result<PreparedSource, SourceError> {
    prepare_from(Path::new(WORK_DIR), REPOSITORY, selector, runner).await
}

async fn prepare_from(
    root: &Path,
    repository: &str,
    selector: &Selector,
    runner: &dyn CommandRunner,
) -> Result<PreparedSource, SourceError> {
    reject_symlinks(root)?;
    std::fs::create_dir_all(root)?;
    let root = root.canonicalize()?;
    // The cache ref/FETCH_HEAD is mutable: serialize even callers outside the CLI.
    let _lock = crate::lock::RunLock::acquire(&root)?;
    ignore_managed_contents(&root)?;
    let cache = cache(&root, runner).await?;
    let reference = format!("+{}:refs/mortsom/selected", selector.git_ref());
    run_git(
        runner,
        &cache,
        &["fetch", "--no-tags", "--force", repository, &reference],
    )
    .await?;
    let revision = run_git(
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

async fn checkout(
    root: &Path,
    cache: &Path,
    repository: &str,
    selector: &Selector,
    revision: &str,
    runner: &dyn CommandRunner,
) -> Result<PreparedSource, SourceError> {
    let directory = tempfile::Builder::new().prefix("build-").tempdir_in(root)?;
    let path = directory.path().join("source");
    let cache = cache
        .to_str()
        .ok_or_else(|| SourceError::Verification("cache path must be UTF-8".into()))?;
    run_git(
        runner,
        directory.path(),
        &["clone", "--local", "--no-checkout", "--", cache, "source"],
    )
    .await?;
    run_git(runner, &path, &["checkout", "--detach", revision, "--"]).await?;
    if run_git(runner, &path, &["rev-parse", "HEAD"]).await?.trim() != revision
        || !run_git(
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
    tracing::info!(revision, selector = %selector.requested, "prepared app source");
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

async fn cache(root: &Path, runner: &dyn CommandRunner) -> Result<PathBuf, SourceError> {
    let cache = root.join("repository.git");
    reject_symlinks(&cache)?;
    let marker = "mortsom-managed-app-cache-v1\n";
    if !cache.exists() {
        let staging = tempfile::Builder::new().prefix("init-").tempdir_in(root)?;
        run_git(runner, staging.path(), &["init", "--bare"]).await?;
        std::fs::write(staging.path().join("mortsom-owner"), marker)?;
        std::fs::rename(staging.path(), &cache)?;
    }
    let owner = cache.join("mortsom-owner");
    reject_symlinks(&owner)?;
    if std::fs::read_to_string(&owner).ok().as_deref() != Some(marker) {
        return Err(SourceError::Ownership(cache));
    }
    if run_git(runner, &cache, &["rev-parse", "--is-bare-repository"])
        .await?
        .trim()
        != "true"
    {
        return Err(SourceError::Ownership(cache));
    }
    let identity = run_git(runner, &cache, &["rev-parse", "--absolute-git-dir"]).await?;
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

async fn run_git(
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
        SourceError::Git(mortsom_common::Redactor::default().redact(&error.to_string()))
    })
}

#[cfg(test)]
#[path = "app_source_tests.rs"]
mod tests;
