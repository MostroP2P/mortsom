//! Explicit source selection applied before validating the effective configuration.
use std::path::PathBuf;

use crate::app_source::Selector;

/// An explicit source selection that replaces both configured source fields.
#[derive(Debug, Clone)]
pub enum AppSource {
    /// Fetch this validated remote branch or pull-request head.
    Remote(Selector),
    /// Build this local checkout, preserving its developer changes.
    Local(PathBuf),
}

impl AppSource {
    /// Discards unused source references before environment resolution and parsing.
    pub(super) fn remove_configured(&self, raw: &mut toml::Value) {
        if let Some(mobile) = raw.get_mut("mobile").and_then(toml::Value::as_table_mut) {
            mobile.remove("source_dir");
            mobile.remove("app_ref");
        }
    }

    /// CLI values are literal; only values from TOML undergo `env:` resolution.
    pub(crate) fn apply(&self, config: &mut super::Config) {
        match self {
            Self::Remote(selector) => {
                config.mobile.source_dir.clear();
                config.mobile.app_ref = Some(selector.to_string());
            }
            Self::Local(path) => {
                config.mobile.source_dir = path.display().to_string();
                config.mobile.app_ref = None;
            }
        }
    }
}
