//! The scan of a process table for a process that may still run with an
//! actor's store; see [`any_process_uses`].

use std::ffi::OsStr;
use std::io::ErrorKind;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::MetadataExt;
use std::path::Path;

use rustix::io::Errno;
use rustix::process::Uid;

use crate::process::EXECUTABLE;

/// Whether any process in the process table at `proc_root` may run with
/// `MCLI_DIR=<mcli>`.
///
/// A process whose environment can be read uses the store only when the
/// environment holds exactly that variable. One whose environment cannot be
/// read (it made itself non-dumpable, as ssh-agent and gpg-agent do, or its
/// executable is not readable) proves nothing; it counts when `uid`, the
/// user Mortsom runs the CLI as, is among its users (see [`runs_as`]) and
/// its first argument names the [`EXECUTABLE`] or its command line cannot
/// be read either. A process that has exited does not count, and a process
/// table that cannot be listed counts as in use. The table must list every
/// process of `uid` for an absent one to prove anything; the caller checks
/// that.
pub(super) fn any_process_uses(proc_root: &Path, uid: Uid, mcli: &Path) -> bool {
    let wanted = format!("MCLI_DIR={}", mcli.display());
    let Ok(mut entries) = std::fs::read_dir(proc_root) else {
        // Without a process table nothing can be proved: assume in use.
        return true;
    };
    entries.any(|entry| match entry {
        // An entry that cannot be listed may be the one that uses it.
        Err(_) => true,
        Ok(entry) => is_pid(&entry.file_name()) && uses(&entry.path(), uid, wanted.as_bytes()),
    })
}

fn is_pid(name: &OsStr) -> bool {
    !name.is_empty() && name.as_bytes().iter().all(u8::is_ascii_digit)
}

/// Whether the process at `process` may run with the `wanted` variable.
fn uses(process: &Path, uid: Uid, wanted: &[u8]) -> bool {
    match std::fs::read(process.join("environ")) {
        Ok(environ) => environ.split(|byte| *byte == 0).any(|var| var == wanted),
        Err(error) if gone(&error) => false,
        Err(_) => runs_as(process, uid) && may_be_the_cli(process),
    }
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
