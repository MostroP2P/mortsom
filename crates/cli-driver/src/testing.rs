//! A scripted mostro-cli for tests: answers each dotted command with a
//! queued outcome document, records every request (with its standard input,
//! so tests can prove a secret travelled there and nowhere else).

use std::collections::{BTreeMap, VecDeque};
use std::sync::Mutex;
use std::time::Duration;

use async_trait::async_trait;

use crate::process::{CliProcess, ProcessFailed, ProcessFailure, ProcessOutput, ProcessRequest};

/// One recorded request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SeenRequest {
    /// Dotted command name read from the arguments.
    pub command: String,
    /// Full argument vector.
    pub args: Vec<String>,
    /// Environment handed to the child.
    pub env: BTreeMap<String, String>,
    /// Standard input, exposed for assertions.
    pub stdin: Option<String>,
}

/// Answers by dotted command name; unscripted commands time out.
#[derive(Debug, Default)]
pub struct ScriptedCli {
    answers: Mutex<BTreeMap<String, VecDeque<Result<ProcessOutput, ProcessFailed>>>>,
    sticky: Mutex<BTreeMap<String, ProcessOutput>>,
    seen: Mutex<Vec<SeenRequest>>,
}

impl ScriptedCli {
    /// Queues a success document with `data` for the next `command`.
    pub fn ok(&self, command: &str, data: serde_json::Value) {
        self.queue(command, Ok(document(command, "ok", &data, None, 0)));
    }

    /// Queues a failure with `code` (`status` is `error` or `uncertain`).
    pub fn fail(&self, command: &str, status: &str, code: &str) {
        self.queue(command, Ok(failure(command, status, code, None)));
    }

    /// Queues a `node_refused` error carrying the node's `reason`.
    pub fn refuse(&self, command: &str, reason: &str) {
        self.queue(
            command,
            Ok(failure(command, "error", "node_refused", Some(reason))),
        );
    }

    /// Queues a raw process answer.
    pub fn raw(&self, command: &str, answer: Result<ProcessOutput, ProcessFailed>) {
        self.queue(command, answer);
    }

    /// Answers every `command` with `data` once its queue is empty.
    pub fn always(&self, command: &str, data: serde_json::Value) {
        lock(&self.sticky).insert(command.to_owned(), document(command, "ok", &data, None, 0));
    }

    /// Every request seen so far.
    pub fn seen(&self) -> Vec<SeenRequest> {
        lock(&self.seen).clone()
    }

    /// The dotted names of every request, in order.
    pub fn commands(&self) -> Vec<String> {
        self.seen().into_iter().map(|r| r.command).collect()
    }

    fn queue(&self, command: &str, answer: Result<ProcessOutput, ProcessFailed>) {
        lock(&self.answers)
            .entry(command.to_owned())
            .or_default()
            .push_back(answer);
    }
}

#[async_trait]
impl CliProcess for ScriptedCli {
    async fn run(
        &self,
        request: &ProcessRequest,
        timeout: Duration,
    ) -> Result<ProcessOutput, ProcessFailed> {
        let command = command_of(&request.args);
        lock(&self.seen).push(SeenRequest {
            command: command.clone(),
            args: request.args.clone(),
            env: request.env.clone(),
            stdin: request.stdin.as_ref().map(|s| s.expose().to_owned()),
        });
        if let Some(answer) = lock(&self.answers)
            .get_mut(&command)
            .and_then(VecDeque::pop_front)
        {
            return answer;
        }
        match lock(&self.sticky).get(&command) {
            Some(output) => Ok(output.clone()),
            None => Err(ProcessFailure::Timeout(timeout).into()),
        }
    }
}

/// The dotted command name: the first two words that are neither flags nor
/// flag values.
pub fn command_of(args: &[String]) -> String {
    let mut words = Vec::new();
    let mut skip = false;
    for arg in args {
        if skip {
            skip = false;
            continue;
        }
        if arg.starts_with("--") {
            skip = arg != "--yes";
            continue;
        }
        words.push(arg.clone());
        if words.len() == 2 {
            break;
        }
    }
    words.join(".")
}

/// A failure document with `code` and, optionally, the node's `reason`.
pub fn failure(command: &str, status: &str, code: &str, reason: Option<&str>) -> ProcessOutput {
    let exit = if code == "invalid_arguments" { 2 } else { 1 };
    let mut output = document(command, status, &serde_json::Value::Null, Some(code), exit);
    if let Some(reason) = reason {
        let mut body: serde_json::Value =
            serde_json::from_str(&output.stdout).unwrap_or(serde_json::Value::Null);
        body["error"]["reason"] = serde_json::Value::String(reason.to_owned());
        output.stdout = body.to_string();
    }
    output
}

/// An outcome document as the contract prints it.
pub fn document(
    command: &str,
    status: &str,
    data: &serde_json::Value,
    code: Option<&str>,
    exit: i32,
) -> ProcessOutput {
    let error = code.map(|code| serde_json::json!({"code": code, "message": "scripted"}));
    let body = serde_json::json!({
        "schema_version": "1",
        "command": command,
        "status": status,
        "data": data,
        "error": error,
        "warnings": [],
        "meta": {},
    });
    ProcessOutput {
        code: Some(exit),
        stdout: body.to_string(),
        stderr: String::new(),
    }
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_command_name_skips_flags_and_their_values() {
        let args: Vec<String> = [
            "--mostro-pubkey",
            "pk",
            "--output",
            "json",
            "order",
            "take-sell",
            "--order",
            "o1",
        ]
        .iter()
        .map(|s| (*s).to_owned())
        .collect();

        assert_eq!(command_of(&args), "order.take-sell");
    }
}
