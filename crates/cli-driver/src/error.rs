//! Failures of one CLI invocation, each with an explicit failure category
//! decided from the contract's stable error code, never from prose.

use mortsom_common::FailureCategory;
use thiserror::Error;

use crate::process::ProcessFailure;

/// Why an invocation produced no usable outcome.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum CliError {
    /// The executable could not run to completion.
    #[error("`{command}` could not run: {cause}")]
    Process {
        /// Dotted command name.
        command: String,
        /// What went wrong.
        cause: ProcessFailure,
    },
    /// The CLI answered outside its machine-interface contract.
    #[error("`{command}` broke the machine-interface contract: {detail}")]
    Contract {
        /// Dotted command name.
        command: String,
        /// What was wrong with the answer.
        detail: String,
    },
    /// The command failed with a documented code.
    #[error("`{command}` failed with `{code}`{}: {message}", reason_suffix(.reason))]
    Refused {
        /// Dotted command name.
        command: String,
        /// Stable error code.
        code: String,
        /// Fixed prose of the code.
        message: String,
        /// The node's reason for a refusal, when it gave one.
        reason: Option<String>,
    },
    /// A request was published and its outcome is unknown. Never retried.
    #[error("`{command}` may have reached the node (`{code}`): {message}")]
    Uncertain {
        /// Dotted command name.
        command: String,
        /// Stable error code.
        code: String,
        /// Fixed prose of the code.
        message: String,
    },
    /// The CLI failed without printing an outcome: it crashed.
    #[error("`{command}` crashed with exit code {exit} and no outcome document")]
    Crashed {
        /// Dotted command name.
        command: String,
        /// Exit code.
        exit: i32,
    },
    /// The actor's private CLI storage could not be prepared or used.
    #[error("CLI storage: {0}")]
    Storage(String),
}

impl CliError {
    /// The failure category of this error.
    pub fn category(&self) -> FailureCategory {
        match self {
            CliError::Process { .. } | CliError::Storage(_) => FailureCategory::TestInfrastructure,
            CliError::Contract { .. } => FailureCategory::AutomationContract,
            // The CLI is the system under test: its crash is a product failure.
            CliError::Crashed { .. } => FailureCategory::Product,
            CliError::Refused { code, .. } => category_of_code(code),
            CliError::Uncertain { .. } => FailureCategory::Relay,
        }
    }

    /// The contract's error code, when the CLI gave one.
    pub fn code(&self) -> Option<&str> {
        match self {
            CliError::Refused { code, .. } | CliError::Uncertain { code, .. } => Some(code),
            _ => None,
        }
    }

    /// The node's reason for a refusal, when it gave one.
    pub fn reason(&self) -> Option<&str> {
        match self {
            CliError::Refused { reason, .. } => reason.as_deref(),
            _ => None,
        }
    }
}

fn reason_suffix(reason: &Option<String>) -> String {
    reason
        .as_deref()
        .map(|reason| format!(" ({reason})"))
        .unwrap_or_default()
}

/// Maps a stable error code to a failure category. The CLI is the system
/// under test, so its own malfunctions and the node's refusals are product
/// failures; an argument it does not understand means Mortsom and the CLI
/// disagree on the contract.
pub fn category_of_code(code: &str) -> FailureCategory {
    match code {
        "invalid_arguments" | "output_not_supported" | "protocol_unsupported" => {
            FailureCategory::AutomationContract
        }
        "relay_connection_failed"
        | "relay_read_failed"
        | "read_timeout"
        | "read_limit_exceeded"
        | "reply_timeout"
        | "request_rejected"
        | "pow_required" => FailureCategory::Relay,
        "configuration_missing"
        | "configuration_invalid"
        | "identity_missing"
        | "identity_unreadable"
        | "identity_exists"
        | "store_unwritable"
        | "store_unreadable" => FailureCategory::EnvironmentMisconfiguration,
        _ => FailureCategory::Product,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn refused(code: &str) -> CliError {
        CliError::Refused {
            command: "order.take-sell".into(),
            code: code.into(),
            message: "fixed".into(),
            reason: None,
        }
    }

    #[test]
    fn a_refusal_names_the_nodes_reason() {
        let error = CliError::Refused {
            command: "order.cancel".into(),
            code: "node_refused".into(),
            message: "The node refused the request.".into(),
            reason: Some("order_already_canceled".into()),
        };

        assert_eq!(error.reason(), Some("order_already_canceled"));
        assert!(error.to_string().contains("(order_already_canceled)"));
    }

    #[test]
    fn categories_come_from_the_code_never_from_prose() {
        assert_eq!(refused("node_refused").category(), FailureCategory::Product);
        assert_eq!(
            refused("reply_malformed").category(),
            FailureCategory::Product
        );
        assert_eq!(
            refused("invalid_arguments").category(),
            FailureCategory::AutomationContract
        );
        assert_eq!(refused("reply_timeout").category(), FailureCategory::Relay);
        assert_eq!(
            refused("identity_missing").category(),
            FailureCategory::EnvironmentMisconfiguration
        );
    }

    #[test]
    fn process_contract_and_uncertain_failures_have_fixed_categories() {
        let process = CliError::Process {
            command: "order.new".into(),
            cause: ProcessFailure::Timeout(Duration::from_secs(1)),
        };
        let contract = CliError::Contract {
            command: "order.new".into(),
            detail: "x".into(),
        };
        let uncertain = CliError::Uncertain {
            command: "order.new".into(),
            code: "reply_timeout".into(),
            message: "x".into(),
        };
        assert_eq!(process.category(), FailureCategory::TestInfrastructure);
        assert_eq!(contract.category(), FailureCategory::AutomationContract);
        assert_eq!(uncertain.category(), FailureCategory::Relay);
        assert_eq!(uncertain.code(), Some("reply_timeout"));
        assert_eq!(process.code(), None);
    }
}
