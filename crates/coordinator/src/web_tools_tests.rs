#![allow(clippy::unwrap_used)]
use super::*;
use crate::doctor::{CommandOutput, HostProbe};
use async_trait::async_trait;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Default)]
struct Host {
    env: BTreeMap<&'static str, &'static str>,
    on_path: BTreeMap<&'static str, &'static str>,
    executables: BTreeSet<&'static str>,
}

#[async_trait]
impl HostProbe for Host {
    fn env(&self, name: &str) -> Option<String> {
        self.env.get(name).map(|v| (*v).to_owned())
    }
    fn which(&self, program: &str) -> Option<PathBuf> {
        self.on_path.get(program).map(PathBuf::from)
    }
    fn exists(&self, path: &Path) -> bool {
        self.is_executable(path)
    }
    fn is_file(&self, path: &Path) -> bool {
        self.is_executable(path)
    }
    fn is_executable(&self, path: &Path) -> bool {
        self.executables.contains(path.to_str().unwrap())
    }
    async fn run(&self, _: &Path, _: &[&str]) -> Result<CommandOutput, String> {
        Err("not used".into())
    }
}

const SNAP_CHROME: &str = "/snap/chromium/current/usr/lib/chromium-browser/chrome";
const SNAP_DRIVER: &str = "/snap/chromium/current/usr/lib/chromium-browser/chromedriver";

#[test]
fn a_snap_wrapper_on_path_is_replaced_by_the_unconfined_pair_inside_the_snap() {
    // Arrange: the operator installed Chromium from snap and nothing else.
    let host = Host {
        on_path: [("chromium", "/snap/bin/chromium")].into(),
        executables: [SNAP_CHROME, SNAP_DRIVER].into(),
        ..Host::default()
    };

    // Act
    let tools = resolve(&host);

    // Assert
    assert_eq!(tools.browser.path, PathBuf::from(SNAP_CHROME));
    assert_eq!(tools.browser.origin, Origin::SnapUnconfined);
    assert_eq!(tools.chromedriver.path, PathBuf::from(SNAP_DRIVER));
    assert_eq!(tools.chromedriver.origin, Origin::SnapUnconfined);
}

#[test]
fn a_snap_chromedriver_symlink_on_path_is_also_replaced_by_the_snap_pair() {
    let host = Host {
        on_path: [
            ("chromium", "/snap/bin/chromium"),
            ("chromedriver", "/home/op/.local/bin/chromedriver"),
        ]
        .into(),
        executables: [SNAP_CHROME, SNAP_DRIVER, "/home/op/.local/bin/chromedriver"].into(),
        ..Host::default()
    };

    let tools = resolve(&host);

    assert_eq!(tools.chromedriver.path, PathBuf::from(SNAP_DRIVER));
    assert_eq!(tools.chromedriver.origin, Origin::SnapUnconfined);
}

#[test]
fn explicit_variables_win_over_detection() {
    let host = Host {
        env: [
            ("MORTSOM_BROWSER", "/opt/chrome/chrome"),
            ("MORTSOM_CHROMEDRIVER", "driver-on-path"),
        ]
        .into(),
        on_path: [
            ("chromium", "/snap/bin/chromium"),
            ("driver-on-path", "/usr/local/bin/driver-on-path"),
        ]
        .into(),
        executables: [SNAP_CHROME, SNAP_DRIVER, "/opt/chrome/chrome"].into(),
    };

    let tools = resolve(&host);

    assert_eq!(tools.browser.path, PathBuf::from("/opt/chrome/chrome"));
    assert_eq!(tools.browser.origin, Origin::Configured);
    assert_eq!(
        tools.chromedriver.path,
        PathBuf::from("/usr/local/bin/driver-on-path")
    );
    assert_eq!(tools.chromedriver.origin, Origin::Configured);
}

#[test]
fn a_regular_install_on_path_is_used_as_is() {
    let host = Host {
        on_path: [
            ("google-chrome", "/usr/bin/google-chrome"),
            ("chromedriver", "/usr/bin/chromedriver"),
        ]
        .into(),
        executables: ["/usr/bin/google-chrome", "/usr/bin/chromedriver"].into(),
        ..Host::default()
    };

    let tools = resolve(&host);

    assert_eq!(tools.browser.path, PathBuf::from("/usr/bin/google-chrome"));
    assert_eq!(tools.browser.origin, Origin::OnPath);
    assert_eq!(
        tools.chromedriver.path,
        PathBuf::from("/usr/bin/chromedriver")
    );
    assert_eq!(tools.chromedriver.origin, Origin::OnPath);
}

#[test]
fn a_snap_wrapper_without_the_unconfined_binaries_is_reported_as_confined() {
    let host = Host {
        on_path: [("chromium", "/snap/bin/chromium")].into(),
        ..Host::default()
    };

    let tools = resolve(&host);

    assert_eq!(tools.browser.path, PathBuf::from("/snap/bin/chromium"));
    assert_eq!(tools.browser.origin, Origin::SnapConfined);
    assert_eq!(tools.chromedriver.origin, Origin::Missing);
    assert!(
        tools.browser.describe().contains("confined"),
        "{}",
        tools.browser.describe()
    );
}

#[test]
fn nothing_found_keeps_the_bare_names_so_errors_stay_readable() {
    let tools = resolve(&Host::default());

    assert_eq!(tools.browser.path, PathBuf::from("chromium"));
    assert_eq!(tools.browser.origin, Origin::Missing);
    assert_eq!(tools.chromedriver.path, PathBuf::from("chromedriver"));
    assert!(
        tools
            .chromedriver
            .describe()
            .contains("MORTSOM_CHROMEDRIVER")
    );
}
