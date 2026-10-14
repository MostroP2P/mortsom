//! Why a run returned no summary, and whether it had started.
//!
//! A run starts when its id and artifacts directory exist. Before that a
//! failure is a refusal: nothing ran, nothing needs cleaning up, and the
//! same refusal holds for the next scenario. After it the run may already
//! have executed and torn down its scenarios, so its id is what the
//! operator needs to read its artifacts or recover it.

use std::fmt;

use mortsom_common::RunId;

/// A run that ended without a summary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunError {
    /// Refused before a run id or artifacts directory existed.
    NotStarted(String),
    /// Failed after the run had its id and artifacts directory.
    Started {
        /// The run, whose artifacts directory exists.
        run_id: RunId,
        /// What failed.
        message: String,
    },
}

impl fmt::Display for RunError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RunError::NotStarted(message) => f.write_str(message),
            RunError::Started { run_id, message } => write!(f, "run {run_id}: {message}"),
        }
    }
}

impl std::error::Error for RunError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_refusal_reads_as_its_reason() {
        // Arrange
        let error = RunError::NotStarted("unknown scenario `nope`".into());

        // Act / Assert
        assert_eq!(error.to_string(), "unknown scenario `nope`");
    }

    #[test]
    fn a_started_run_names_itself() {
        // Arrange
        let run_id = RunId::new();
        let error = RunError::Started {
            run_id: run_id.clone(),
            message: "cannot write summary.json".into(),
        };

        // Act / Assert
        assert_eq!(
            error.to_string(),
            format!("run {run_id}: cannot write summary.json")
        );
    }
}
