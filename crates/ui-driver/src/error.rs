//! Failures of a UI session, whichever client is behind it.

use std::time::Duration;

use thiserror::Error;

use crate::process::ProcessError;
use crate::webdriver::WebDriverError;

/// Longest background interval a session accepts.
pub const MAX_BACKGROUND_SECS: u64 = 300;

/// UI session failures.
#[derive(Debug, Error)]
pub enum UiError {
    /// A client or helper process could not be started or died.
    #[error(transparent)]
    Process(#[from] ProcessError),
    /// A driver server did not answer in time.
    #[error("driver at {url} not ready after {timeout:?}")]
    NotReady {
        /// Base URL probed.
        url: String,
        /// Deadline used.
        timeout: Duration,
    },
    /// A WebDriver call failed.
    #[error(transparent)]
    WebDriver(#[from] WebDriverError),
    /// A background interval exceeded [`MAX_BACKGROUND_SECS`].
    #[error("background of {0}s exceeds the maximum of {MAX_BACKGROUND_SECS}s")]
    BackgroundTooLong(u64),
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
