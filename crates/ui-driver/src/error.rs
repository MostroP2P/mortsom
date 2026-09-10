//! Failures of a UI session, whichever client is behind it.

use std::time::Duration;

use thiserror::Error;

use crate::process::ProcessError;
use crate::webdriver::WebDriverError;

/// UI session failures.
#[derive(Debug, Error)]
pub enum UiError {
    /// A client or helper process could not be started or died.
    #[error(transparent)]
    Process(#[from] ProcessError),
    /// A WebDriver call failed.
    #[error(transparent)]
    WebDriver(#[from] WebDriverError),
    /// An element did not appear (or disappear) before the deadline.
    #[error("element `{accessibility_id}` {condition} within {timeout:?}")]
    WaitTimeout {
        /// Accessibility id waited on.
        accessibility_id: String,
        /// `not found` or `still present`.
        condition: &'static str,
        /// Deadline used.
        timeout: Duration,
    },
}
