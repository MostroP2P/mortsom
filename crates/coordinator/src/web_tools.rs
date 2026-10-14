//! Locate the Chromium and ChromeDriver pair that drives Web clients.
//!
//! The pair must match versions, and the browser must be allowed to write its
//! profile under `~/.local/state/mortsom`. The common Ubuntu install is the
//! Chromium snap, whose `/snap/bin/chromium` wrapper runs confined and cannot
//! write hidden directories in the home, so the browser exits at launch with
//! nothing more helpful than "Chrome instance exited". The same snap ships
//! unconfined `chrome` and `chromedriver` binaries that work; this module
//! prefers them whenever the wrapper is what `PATH` offers. Explicit
//! `MORTSOM_BROWSER` and `MORTSOM_CHROMEDRIVER` values always win.

use std::path::{Path, PathBuf};

use crate::doctor::HostProbe;

/// Variable that names the browser executable.
pub const BROWSER_VARIABLE: &str = "MORTSOM_BROWSER";
/// Variable that names the ChromeDriver executable.
pub const CHROMEDRIVER_VARIABLE: &str = "MORTSOM_CHROMEDRIVER";
/// Where the Chromium snap keeps its unconfined binaries.
pub const SNAP_CHROMIUM_LIB: &str = "/snap/chromium/current/usr/lib/chromium-browser";
/// Snap command wrappers live here and run confined.
const SNAP_BIN: &str = "/snap/bin";
/// Ubuntu's transitional package installs this name as a script that runs
/// the Chromium snap.
const UBUNTU_WRAPPER: &str = "chromium-browser";
/// Browser names tried on `PATH`, in order.
const BROWSER_NAMES: [&str; 4] = [
    "chromium",
    "chromium-browser",
    "google-chrome",
    "google-chrome-stable",
];
/// Driver name tried on `PATH`.
const CHROMEDRIVER_NAME: &str = "chromedriver";

/// How a tool path was chosen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Origin {
    /// Named by the operator through its environment variable.
    Configured,
    /// Found on `PATH` as a regular executable.
    OnPath,
    /// `PATH` offered the snap wrapper; the unconfined binary inside the snap is used instead.
    SnapUnconfined,
    /// `PATH` offered the snap wrapper and the snap holds no usable unconfined binary.
    SnapConfined,
    /// Nothing found; the bare name is kept so later errors name it.
    Missing,
}

/// One resolved executable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WebTool {
    /// What to execute.
    pub path: PathBuf,
    /// Why this path.
    pub origin: Origin,
    /// Variable that overrides it.
    pub variable: &'static str,
}

impl WebTool {
    /// True when the path is expected to work.
    pub fn is_usable(&self) -> bool {
        !matches!(self.origin, Origin::SnapConfined | Origin::Missing)
    }

    /// Operator-facing account of the choice.
    pub fn describe(&self) -> String {
        let path = self.path.display();
        match self.origin {
            Origin::Configured => format!("{path} (from {})", self.variable),
            Origin::OnPath => path.to_string(),
            Origin::SnapUnconfined => {
                format!(
                    "{path} (unconfined binary inside the Chromium snap; the /snap/bin wrapper cannot write profiles under ~/.local)"
                )
            }
            Origin::SnapConfined => format!(
                "{path} is the confined snap wrapper and cannot write profiles under ~/.local; install the Chromium snap fully or set {}",
                self.variable
            ),
            Origin::Missing => format!(
                "{path} not found; install Chromium or set {}",
                self.variable
            ),
        }
    }
}

/// The browser and driver a Web run will use.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WebTools {
    /// Chromium or Chrome.
    pub browser: WebTool,
    /// Matching ChromeDriver.
    pub chromedriver: WebTool,
}

/// Choose the pair for this host without executing anything.
pub fn resolve(host: &dyn HostProbe) -> WebTools {
    let browser = resolve_browser(host);
    let chromedriver = resolve_chromedriver(host, &browser);
    WebTools {
        browser,
        chromedriver,
    }
}

fn resolve_browser(host: &dyn HostProbe) -> WebTool {
    if let Some(tool) = configured(host, BROWSER_VARIABLE) {
        return tool;
    }
    let Some(found) = BROWSER_NAMES.iter().find_map(|name| host.which(name)) else {
        return missing(BROWSER_NAMES[0], BROWSER_VARIABLE);
    };
    from_path(host, found, "chrome", BROWSER_VARIABLE)
}

fn resolve_chromedriver(host: &dyn HostProbe, browser: &WebTool) -> WebTool {
    if let Some(tool) = configured(host, CHROMEDRIVER_VARIABLE) {
        return tool;
    }
    // A version-matched driver ships next to the unconfined browser.
    if browser.origin == Origin::SnapUnconfined
        && let Some(driver) = snap_binary(host, CHROMEDRIVER_NAME)
    {
        return WebTool {
            path: driver,
            origin: Origin::SnapUnconfined,
            variable: CHROMEDRIVER_VARIABLE,
        };
    }
    let Some(found) = host.which(CHROMEDRIVER_NAME) else {
        return missing(CHROMEDRIVER_NAME, CHROMEDRIVER_VARIABLE);
    };
    from_path(host, found, CHROMEDRIVER_NAME, CHROMEDRIVER_VARIABLE)
}

/// An explicit variable is taken as given: a bare name is looked up on `PATH`,
/// anything else is used verbatim, exactly as before detection existed.
fn configured(host: &dyn HostProbe, variable: &'static str) -> Option<WebTool> {
    let value = host.env(variable)?;
    let path = host.which(&value).unwrap_or_else(|| PathBuf::from(value));
    Some(WebTool {
        path,
        origin: Origin::Configured,
        variable,
    })
}

/// A `PATH` hit is used as is unless it is a snap wrapper.
fn from_path(
    host: &dyn HostProbe,
    found: PathBuf,
    snap_name: &str,
    variable: &'static str,
) -> WebTool {
    if !found.starts_with(SNAP_BIN) {
        // Ubuntu's `chromium-browser` package is a script that hands over to
        // the confined snap; with the snap installed, its unconfined binary
        // is the one that works. Without it the hit may be a real browser
        // (Fedora ships one) and is kept for the doctor's version check.
        if found.file_name().is_some_and(|name| name == UBUNTU_WRAPPER)
            && let Some(path) = snap_binary(host, snap_name)
        {
            return WebTool {
                path,
                origin: Origin::SnapUnconfined,
                variable,
            };
        }
        return WebTool {
            path: found,
            origin: Origin::OnPath,
            variable,
        };
    }
    match snap_binary(host, snap_name) {
        Some(path) => WebTool {
            path,
            origin: Origin::SnapUnconfined,
            variable,
        },
        None => WebTool {
            path: found,
            origin: Origin::SnapConfined,
            variable,
        },
    }
}

fn snap_binary(host: &dyn HostProbe, name: &str) -> Option<PathBuf> {
    let path = Path::new(SNAP_CHROMIUM_LIB).join(name);
    host.is_executable(&path).then_some(path)
}

fn missing(name: &str, variable: &'static str) -> WebTool {
    WebTool {
        path: PathBuf::from(name),
        origin: Origin::Missing,
        variable,
    }
}

#[cfg(test)]
#[path = "web_tools_tests.rs"]
mod tests;
