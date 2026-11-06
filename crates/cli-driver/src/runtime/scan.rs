//! The scan of a process table for a process that may still run with an
//! actor's store; see [`any_process_uses`].

use std::ffi::OsStr;
use std::io::ErrorKind;
use std::os::unix::ffi::OsStrExt;
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
/// executable is not readable) proves nothing; it counts when its real user
/// is `uid`, the user Mortsom runs the CLI as, and its command line either
/// starts with the [`EXECUTABLE`] or cannot be read either. A process that
/// disappears while it is examined does not count, and a process table that
/// cannot be listed counts as in use.
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

/// Whether the process's real user is `uid`. A status that cannot be read
/// belongs to a process that is gone or that another user owns.
pub(super) fn runs_as(process: &Path, uid: Uid) -> bool {
    std::fs::read_to_string(process.join("status"))
        .ok()
        .and_then(|status| real_uid(&status))
        == Some(uid.as_raw())
}

/// The real user, the first of the four on the status's `Uid:` line.
fn real_uid(status: &str) -> Option<u32> {
    status
        .lines()
        .find_map(|line| line.strip_prefix("Uid:"))?
        .split_whitespace()
        .next()?
        .parse()
        .ok()
}

/// Whether the process's command line starts with the CLI executable, or
/// cannot be read while the process still exists.
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
