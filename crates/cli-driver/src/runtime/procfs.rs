//! Whether the procfs at `/proc` can prove a process absent: the scan only
//! proves a stop when `/proc` is the procfs of Mortsom's own PID namespace
//! and lists every process of the operator's user.

use std::os::unix::fs::MetadataExt;
use std::path::Path;

use rustix::fs::{major, minor};
use rustix::process::{getegid, getgroups};

/// Whether the procfs at `proc_root` belongs to the PID namespace of the
/// caller, the process `pid`, and lists every process of the caller's
/// user, so that a process missing from it does not run there. `in_group`
/// says whether the caller is in a group, numbered as the initial user
/// namespace numbers it.
pub(super) fn lists_every_process(
    proc_root: &Path,
    pid: u32,
    in_group: impl Fn(u32) -> bool,
) -> bool {
    names_the_caller(proc_root, pid)
        && mount_options(proc_root).is_some_and(|options| !hides_processes(&options, in_group))
}

/// Whether the procfs's `self` names `pid`: a procfs of another PID
/// namespace names the caller by another number, or not at all.
fn names_the_caller(proc_root: &Path, pid: u32) -> bool {
    std::fs::read_link(proc_root.join("self"))
        .is_ok_and(|target| target.as_os_str() == pid.to_string().as_str())
}

/// The super options of the procfs mounted at `proc_root`, from the
/// caller's own mount table; `None` when no procfs is mounted there, or
/// when that cannot be told.
fn mount_options(proc_root: &Path) -> Option<String> {
    let device = std::fs::metadata(proc_root).ok()?.dev();
    let mountinfo = std::fs::read_to_string(proc_root.join("self/mountinfo")).ok()?;
    let point = proc_root.to_str()?;
    procfs_options(&mountinfo, point, (major(device), minor(device))).map(str::to_owned)
}

/// The super options of the procfs that `mountinfo` shows mounted at
/// `point` as device `(major, minor)`, the device of what `point` shows.
/// Each line reads `ID PARENT MAJOR:MINOR ROOT POINT OPTIONS [TAGS] - TYPE
/// SOURCE SUPER-OPTIONS`, with blanks in names escaped.
fn procfs_options<'a>(
    mountinfo: &'a str,
    point: &str,
    (major, minor): (u32, u32),
) -> Option<&'a str> {
    let device = format!("{major}:{minor}");
    mountinfo.lines().find_map(|line| {
        let (mount, filesystem) = line.split_once(" - ")?;
        let mount: Vec<&str> = mount.split(' ').collect();
        let mut filesystem = filesystem.split(' ');
        let here = mount.get(2) == Some(&device.as_str()) && mount.get(4) == Some(&point);
        (here && filesystem.next() == Some("proc"))
            .then(|| filesystem.nth(1))
            .flatten()
    })
}

/// Whether a procfs with these super options leaves some process of the
/// caller's user out of its listing. `hidepid=invisible` (2) leaves out
/// every process the caller may not trace, a non-dumpable one of its own
/// user included, unless the caller is in the `gid=` group (root's when
/// none is named); `hidepid=ptraceable` (4) does so whatever the group.
/// `noaccess` (1) lists every process and hides only what is inside, which
/// the scan judges by the owner of the process's directory. An unknown
/// mode proves nothing.
fn hides_processes(options: &str, in_group: impl Fn(u32) -> bool) -> bool {
    match option(options, "hidepid=") {
        None | Some("off" | "0" | "noaccess" | "1") => false,
        Some("invisible" | "2") => !option(options, "gid=")
            .map_or(Some(0), |gid| gid.parse().ok())
            .is_some_and(in_group),
        Some(_) => true,
    }
}

/// The value of the last `name` option in a comma-separated list.
fn option<'a>(options: &'a str, name: &str) -> Option<&'a str> {
    options
        .split(',')
        .rev()
        .find_map(|option| option.strip_prefix(name))
}

/// Whether the caller is in the group the initial user namespace numbers
/// `gid`, as the procfs checks it: its effective group or a supplementary
/// one. A mount names its group by that number, and the caller's groups are
/// numbered so only in a user namespace that maps every id to itself;
/// elsewhere no group counts.
pub(super) fn in_group(gid: u32) -> bool {
    let initial = std::fs::read_to_string("/proc/self/gid_map")
        .is_ok_and(|map| maps_every_id_to_itself(&map));
    initial
        && (getegid().as_raw() == gid
            || getgroups().is_ok_and(|groups| groups.iter().any(|group| group.as_raw() == gid)))
}

/// Whether a `gid_map` maps every id to itself, as the initial user
/// namespace's does: one range from 0 to 0 covering every id.
fn maps_every_id_to_itself(map: &str) -> bool {
    map.split_whitespace().eq(["0", "0", "4294967295"])
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use rustix::fs::{major, minor};
    use std::os::unix::fs::MetadataExt;

    const MOUNTINFO: &str = "\
22 1 259:2 / / rw,relatime shared:1 - ext4 /dev/nvme0n1p2 rw
28 22 0:25 / /proc rw,nosuid,nodev,noexec,relatime shared:13 - proc proc rw
29 28 0:26 / /proc/sys/fs/binfmt_misc rw,relatime shared:14 - autofs systemd-1 rw,fd=32
3185 28 0:225 / /proc rw,nosuid,nodev,noexec,relatime - proc proc rw,gid=1000,hidepid=invisible
";

    #[test]
    fn the_options_are_those_of_the_procfs_mounted_there_as_that_device() {
        assert_eq!(procfs_options(MOUNTINFO, "/proc", (0, 25)), Some("rw"));
        assert_eq!(
            procfs_options(MOUNTINFO, "/proc", (0, 225)),
            Some("rw,gid=1000,hidepid=invisible")
        );
        assert_eq!(procfs_options(MOUNTINFO, "/proc", (0, 26)), None);
        assert_eq!(procfs_options(MOUNTINFO, "/", (259, 2)), None);
        assert_eq!(procfs_options(MOUNTINFO, "/proc", (0, 99)), None);
        assert_eq!(
            procfs_options("28 22 0:25 / /proc rw", "/proc", (0, 25)),
            None
        );
    }

    #[test]
    fn only_a_hidepid_that_hides_listed_processes_hides_them() {
        let member = |gid| gid == 1000;
        let nobody = |_| false;
        for options in ["rw", "rw,hidepid=off", "rw,hidepid=0", "rw,subset=pid"] {
            assert!(!hides_processes(options, nobody), "{options}");
        }
        // `noaccess` lists every process; the scan judges what it hides.
        for options in ["rw,hidepid=noaccess", "rw,hidepid=1"] {
            assert!(!hides_processes(options, nobody), "{options}");
        }
        for options in ["rw,gid=1000,hidepid=invisible", "rw,gid=1000,hidepid=2"] {
            assert!(!hides_processes(options, member), "{options}");
            assert!(hides_processes(options, nobody), "{options}");
        }
        // `ptraceable` ignores the group, and so does an unknown mode.
        for options in [
            "rw,gid=1000,hidepid=ptraceable",
            "rw,gid=1000,hidepid=4",
            "rw,gid=1000,hidepid=8",
            "rw,gid=x,hidepid=invisible",
        ] {
            assert!(hides_processes(options, member), "{options}");
        }
    }

    /// With no `gid=`, the procfs exempts the root group.
    #[test]
    fn invisible_without_a_group_exempts_only_the_root_group() {
        assert!(!hides_processes("rw,hidepid=invisible", |gid| gid == 0));
        assert!(hides_processes("rw,hidepid=invisible", |gid| gid == 1000));
    }

    #[test]
    fn only_the_full_identity_map_numbers_groups_as_the_initial_namespace() {
        assert!(maps_every_id_to_itself(
            "         0          0 4294967295\n"
        ));
        assert!(!maps_every_id_to_itself(
            "         0       1000          1\n"
        ));
        assert!(!maps_every_id_to_itself("0 0 1000\n1000 1000 4294966295\n"));
        assert!(!maps_every_id_to_itself(""));
    }

    #[test]
    fn the_caller_s_effective_group_counts_where_groups_are_numbered_alike() {
        let initial = std::fs::read_to_string("/proc/self/gid_map")
            .is_ok_and(|map| maps_every_id_to_itself(&map));

        assert_eq!(in_group(rustix::process::getegid().as_raw()), initial);
    }

    /// A fake procfs: `self` names the caller, and `mountinfo` says what
    /// is mounted at the fake root.
    struct Procfs(tempfile::TempDir);

    impl Procfs {
        fn new(self_pid: Option<&str>, filesystem: &str, options: &str) -> Self {
            let root = tempfile::tempdir().unwrap();
            let device = std::fs::metadata(root.path()).unwrap().dev();
            std::fs::create_dir(root.path().join("4242")).unwrap();
            let line = format!(
                "28 22 {}:{} / {} rw,nosuid shared:13 - {filesystem} proc {options}\n",
                major(device),
                minor(device),
                root.path().display()
            );
            std::fs::write(root.path().join("4242/mountinfo"), line).unwrap();
            if let Some(pid) = self_pid {
                std::os::unix::fs::symlink(pid, root.path().join("self")).unwrap();
            }
            Self(root)
        }

        fn lists_every_process(&self) -> bool {
            lists_every_process(self.0.path(), 4242, |_| false)
        }
    }

    #[test]
    fn a_procfs_of_the_caller_s_namespace_without_hidepid_lists_every_process() {
        assert!(Procfs::new(Some("4242"), "proc", "rw").lists_every_process());
        assert!(Procfs::new(Some("4242"), "proc", "rw,hidepid=noaccess").lists_every_process());
    }

    #[test]
    fn a_procfs_that_hides_processes_does_not() {
        let procfs = Procfs::new(Some("4242"), "proc", "rw,hidepid=invisible");

        assert!(!procfs.lists_every_process());
    }

    /// A procfs of another PID namespace names the caller by another
    /// number, or not at all.
    #[test]
    fn a_procfs_of_another_namespace_does_not() {
        assert!(!Procfs::new(Some("7"), "proc", "rw").lists_every_process());
        assert!(!Procfs::new(None, "proc", "rw").lists_every_process());
    }

    #[test]
    fn something_other_than_a_procfs_does_not() {
        assert!(!Procfs::new(Some("4242"), "tmpfs", "rw").lists_every_process());
    }
}
