//! The scan of a process table for a process that may still run with an
//! actor's store; see [`why_in_use`].

use std::ffi::OsStr;
use std::io::ErrorKind;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::time::Duration;

use rustix::io::Errno;
use rustix::process::Uid;

use crate::process::EXECUTABLE;

/// How long a check of another path to the store may take. A healthy one
/// ends at once; one on an unresponsive mount (an NFS server that went
/// away) can block for as long as the mount hangs.
pub(super) const PATH_CHECK: Duration = Duration::from_secs(2);

/// Why a process may run with the store, or the scan cannot tell.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum InUse {
    /// What `/proc` shows of a process, or fails to show, ties it to the
    /// store; or the process table cannot be listed.
    Process,
    /// A process sets the variable to another absolute path that could not
    /// be checked against the store: its check did not complete within
    /// [`PATH_CHECK`], its metadata could not be read, or the store's own
    /// could not (see [`Identity`]). That path may lead to the store.
    Unchecked(PathBuf),
}

/// Why a process in the process table at `proc_root` may run with
/// `MCLI_DIR=<mcli>`, or `None` when none may.
///
/// A process whose environment can be read uses the store only when the
/// environment sets that variable to the store's path, byte for byte, or to
/// another absolute path to the same directory (a bind mount or a symbolic
/// link); one whose other path cannot be checked against the store (the
/// check does not complete within [`PATH_CHECK`], or that path or the
/// store cannot be read) counts, and one whose path leads nowhere does not.
/// One whose environment cannot be read (it made itself
/// non-dumpable, as ssh-agent and gpg-agent do, or its executable is not
/// readable) proves nothing; it counts when `uid`, the user Mortsom runs
/// the CLI as, is among its users (see [`runs_as`]) and its first argument
/// names the [`EXECUTABLE`] or its command line cannot be read either. A
/// process that has exited does not count, except a thread-group leader
/// whose other threads still run: those are judged the same way. A process
/// table that cannot be listed counts as in use. The table must list every
/// process of `uid` for an absent one to prove anything; the caller checks
/// that.
pub(super) fn why_in_use(proc_root: &Path, uid: Uid, mcli: &Path) -> Option<InUse> {
    scan(proc_root, uid, &Store::new(mcli))
}

/// [`why_in_use`], for the store as `store` names it.
fn scan(proc_root: &Path, uid: Uid, store: &Store) -> Option<InUse> {
    let Ok(mut entries) = std::fs::read_dir(proc_root) else {
        // Without a process table nothing can be proved: assume in use.
        return Some(InUse::Process);
    };
    entries.find_map(|entry| match entry {
        // An entry that cannot be listed may be the one that uses it.
        Err(_) => Some(InUse::Process),
        Ok(entry) if is_pid(&entry.file_name()) => uses(&entry.path(), uid, store, true),
        Ok(_) => None,
    })
}

fn is_pid(name: &OsStr) -> bool {
    !name.is_empty() && name.as_bytes().iter().all(u8::is_ascii_digit)
}

/// The variable that names a CLI's store.
const STORE_VARIABLE: &[u8] = b"MCLI_DIR=";

/// The identity of another path to the store, or [`Incomplete`] when the
/// check does not end in time.
type PathCheck = fn(&Path) -> Result<Identity, Incomplete>;

/// The actor's store as an environment may name it.
struct Store {
    /// `MCLI_DIR=<path>`, as bytes.
    variable: Vec<u8>,
    /// The directory's identity. Unless it is [`Identity::Found`], another
    /// path that leads to a directory cannot be compared with the store.
    /// The store is Mortsom's own, under the root whose marker was just
    /// read, so this check is not bounded.
    identity: Identity,
    /// How another path is checked.
    check: PathCheck,
}

impl Store {
    fn new(mcli: &Path) -> Self {
        let variable = [STORE_VARIABLE, mcli.as_os_str().as_bytes()].concat();
        Self {
            variable,
            identity: identity(mcli),
            check: bounded_identity,
        }
    }

    /// The same store, checking other paths with `check`.
    #[cfg(test)]
    fn checking_paths_with(self, check: PathCheck) -> Self {
        Self { check, ..self }
    }

    /// Why a NUL-separated environment names the store, if it does.
    fn named_in(&self, environ: &[u8]) -> Option<InUse> {
        environ.split(|byte| *byte == 0).find_map(|var| {
            if var == self.variable {
                Some(InUse::Process)
            } else {
                self.reached_by(var)
            }
        })
    }

    /// Why `var` may set the variable to another absolute path to the
    /// store's directory, if it may: the path leads to it, or whether it
    /// does cannot be told. A path that leads nowhere does not, nor one that
    /// leads to another directory while the store's own identity is known;
    /// a relative one is not checked.
    fn reached_by(&self, var: &[u8]) -> Option<InUse> {
        let path = var
            .strip_prefix(STORE_VARIABLE)
            .filter(|path| path.starts_with(b"/"))?;
        let path = Path::new(OsStr::from_bytes(path));
        match ((self.check)(path), self.identity) {
            (Ok(Identity::Absent), _) => None,
            (Ok(found @ Identity::Found(..)), store @ Identity::Found(..)) => {
                (found == store).then_some(InUse::Process)
            }
            // The check did not complete, the path could not be read, or
            // the store has no identity to compare it with.
            (Err(Incomplete) | Ok(Identity::Unverified), _)
            | (Ok(Identity::Found(..)), Identity::Absent | Identity::Unverified) => {
                Some(InUse::Unchecked(path.to_path_buf()))
            }
        }
    }
}

/// What the metadata of a path tells of the directory it leads to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Identity {
    /// Its device and inode.
    Found(u64, u64),
    /// Nothing is there in this mount namespace: the path, or one of its
    /// components, does not exist or is not a directory.
    Absent,
    /// The metadata could not be read (permission denied, an I/O error, a
    /// loop of symbolic links): the path may lead anywhere.
    Unverified,
}

impl Identity {
    /// The identity a read of a path's metadata gives. Only a path that is
    /// not there is [`Identity::Absent`]; any other failure proves nothing.
    fn of(metadata: std::io::Result<std::fs::Metadata>) -> Self {
        match metadata {
            Ok(meta) => Self::Found(meta.dev(), meta.ino()),
            Err(error)
                if matches!(error.kind(), ErrorKind::NotFound | ErrorKind::NotADirectory) =>
            {
                Self::Absent
            }
            Err(_) => Self::Unverified,
        }
    }
}

/// The [`Identity`] of the directory at `path`.
fn identity(path: &Path) -> Identity {
    Identity::of(std::fs::metadata(path))
}

/// [`identity`], bounded by [`PATH_CHECK`].
fn bounded_identity(path: &Path) -> Result<Identity, Incomplete> {
    let path = path.to_path_buf();
    bounded(PATH_CHECK, move || identity(&path))
}

/// A check that did not complete within its bound.
#[derive(Debug, PartialEq, Eq)]
pub(super) struct Incomplete;

/// Runs `check` on a thread of its own and waits at most `limit` for its
/// answer. A stat on a hung mount sleeps in the kernel and cannot be
/// cancelled: its thread is left behind, blocked, and ends if the call ever
/// returns. The scan stops at the first incomplete check, so each scan
/// leaves at most one such thread. A check that cannot be started, or ends
/// without an answer, is incomplete too.
fn bounded<T: Send + 'static>(
    limit: Duration,
    check: impl FnOnce() -> T + Send + 'static,
) -> Result<T, Incomplete> {
    let (answer, answered) = std::sync::mpsc::sync_channel(1);
    let started = std::thread::Builder::new()
        .name("mortsom-path-check".into())
        .spawn(move || {
            // The scan may have stopped waiting: the answer is dropped.
            let _ = answer.send(check());
        });
    if let Err(error) = started {
        tracing::warn!(%error, "could not start a check of another path to a CLI store");
        return Err(Incomplete);
    }
    answered.recv_timeout(limit).map_err(|_| Incomplete)
}

/// Why the process or thread at `dir` may run with the store, if it may;
/// `leader` says it is a process the table lists, not one of its threads.
fn uses(dir: &Path, uid: Uid, store: &Store, leader: bool) -> Option<InUse> {
    match std::fs::read(dir.join("environ")) {
        Ok(environ) if !environ.is_empty() => store.named_in(&environ),
        Err(error) if !gone(&error) => {
            (runs_as(dir, uid) && may_be_the_cli(dir)).then_some(InUse::Process)
        }
        // Gone or empty: a leader that exited before its other threads is a
        // zombie whose own environment reads so while they still run.
        _ if leader => a_thread_uses(dir, uid, store),
        _ => None,
    }
}

/// Why a thread of the process may run with the store, when its status
/// counts more than one.
fn a_thread_uses(process: &Path, uid: Uid, store: &Store) -> Option<InUse> {
    match std::fs::read_to_string(process.join("status")) {
        Ok(status) if thread_count(&status).is_some_and(|threads| threads <= 1) => None,
        Ok(_) => match std::fs::read_dir(process.join("task")) {
            Ok(mut threads) => threads.find_map(|thread| match thread {
                Err(_) => Some(InUse::Process),
                Ok(thread) => uses(&thread.path(), uid, store, false),
            }),
            Err(error) => still_there(&error),
        },
        Err(error) => still_there(&error),
    }
}

/// A read that failed while the process still exists proves nothing, so
/// the process counts.
fn still_there(error: &std::io::Error) -> Option<InUse> {
    (!gone(error)).then_some(InUse::Process)
}

/// The thread count on a status's `Threads:` line.
fn thread_count(status: &str) -> Option<u32> {
    status
        .lines()
        .find_map(|line| line.strip_prefix("Threads:"))?
        .trim()
        .parse()
        .ok()
}

/// Whether `uid` is among the process's users. Its status names all four
/// (real, effective, saved, filesystem), and one without a readable `Uid:`
/// line proves nothing, so the process counts. A status that is gone means
/// the process is. A status that is hidden (`/proc` mounted with
/// `hidepid=noaccess` hides it from the process's own user when the process
/// is non-dumpable) leaves the owner `/proc` gives the process's directory,
/// its effective user, which `/proc` shows whatever the process's state.
pub(super) fn runs_as(process: &Path, uid: Uid) -> bool {
    match std::fs::read_to_string(process.join("status")) {
        Ok(status) => names_user(&status, uid),
        Err(error) if gone(&error) => false,
        Err(_) => owned_by(std::fs::metadata(process).map(|meta| meta.uid()), uid),
    }
}

/// Whether a status's `Uid:` line names `uid`, or cannot be read.
fn names_user(status: &str, uid: Uid) -> bool {
    let users: Option<Vec<u32>> = status
        .lines()
        .find_map(|line| line.strip_prefix("Uid:"))
        .and_then(|users| {
            users
                .split_whitespace()
                .map(|user| user.parse().ok())
                .collect()
        });
    users.is_none_or(|users| users.is_empty() || users.contains(&uid.as_raw()))
}

/// Whether the owner of a process's directory is `uid`. An owner that
/// cannot be read while the process still exists proves nothing, so the
/// process counts.
fn owned_by(owner: std::io::Result<u32>, uid: Uid) -> bool {
    match owner {
        Ok(owner) => owner == uid.as_raw(),
        Err(error) => !gone(&error),
    }
}

/// Whether the process's first argument names the CLI executable, or its
/// command line cannot be read while the process still exists.
pub(super) fn may_be_the_cli(process: &Path) -> bool {
    match std::fs::read(process.join("cmdline")) {
        Ok(cmdline) => {
            let program = cmdline.split(|byte| *byte == 0).next().unwrap_or_default();
            Path::new(OsStr::from_bytes(program)).file_name() == Some(OsStr::new(EXECUTABLE))
        }
        Err(error) => !gone(&error),
    }
}

/// Whether a read failed because the process no longer exists.
pub(super) fn gone(error: &std::io::Error) -> bool {
    error.kind() == ErrorKind::NotFound || error.raw_os_error() == Some(Errno::SRCH.raw_os_error())
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
#[path = "scan_tests.rs"]
mod tests;
