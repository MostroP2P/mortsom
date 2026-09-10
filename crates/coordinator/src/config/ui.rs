//! Interaction deadlines shared by Linux and Web.
use serde::{Deserialize, Serialize};

/// Optional `[ui]` settings.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UiConfig {
    /// Bounded wait for one UI interaction; defaults to thirty seconds on desktop.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_secs: Option<u64>,
}
