//! What one actor's invocations leave behind: the sanitized transcript and
//! the CLI's standard error. The task that runs an invocation holds a clone
//! and records the outcome itself, so a caller that gives up (a scenario
//! deadline) still leaves the command and its result in the evidence.

use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use mortsom_common::Secret;

use crate::call::Call;
use crate::envelope::{Envelope, Status};
use crate::error::CliError;
use crate::process::{ProcessFailure, ProcessOutput};
use crate::transcript::{Recorded, Transcript, scrub};

/// How much of the CLI's standard error is kept per actor.
const STDERR_LIMIT: usize = 256 * 1024;

/// One actor's transcript and standard error, shared with running
/// invocations.
#[derive(Debug, Clone, Default)]
pub(crate) struct Evidence {
    transcript: Arc<Mutex<Transcript>>,
    stderr: Arc<Mutex<String>>,
}

impl Evidence {
    /// Records one finished invocation and returns its `ok` outcome.
    pub(crate) fn interpret(
        &self,
        call: &Call,
        argv: &[String],
        elapsed_ms: u64,
        result: Result<ProcessOutput, ProcessFailure>,
    ) -> Result<Envelope, CliError> {
        let command = call.name();
        let output = match result {
            Ok(output) => output,
            Err(cause) => {
                let what = Recorded::Failure(cause.to_string(), None);
                self.record(&command, argv, elapsed_ms, what);
                return Err(CliError::Process { command, cause });
            }
        };
        let secrets: Vec<&Secret> = call.stdin().into_iter().collect();
        self.keep_stderr(&scrub(&output.stderr, &secrets));
        if let Some(error) = without_document(&command, &output) {
            let what = Recorded::Failure(error.to_string(), output.code);
            self.record(&command, argv, elapsed_ms, what);
            return Err(error);
        }
        let envelope = match Envelope::parse(&output.stdout, output.code) {
            Ok(envelope) => envelope,
            Err(detail) => {
                let detail = scrub(&detail, &secrets);
                let what = Recorded::Failure(detail.clone(), output.code);
                self.record(&command, argv, elapsed_ms, what);
                return Err(CliError::Contract { command, detail });
            }
        };
        let what = Recorded::Outcome(&envelope, output.code);
        self.record(&command, argv, elapsed_ms, what);
        if envelope.command != command {
            let detail = format!("answered as `{}`", envelope.command);
            return Err(CliError::Contract { command, detail });
        }
        outcome(command, envelope)
    }

    fn record(&self, command: &str, argv: &[String], elapsed_ms: u64, what: Recorded<'_>) {
        lock(&self.transcript).record(command, argv, elapsed_ms, what);
    }

    fn keep_stderr(&self, stderr: &str) {
        if stderr.is_empty() {
            return;
        }
        let mut kept = lock(&self.stderr);
        kept.push_str(stderr);
        if !stderr.ends_with('\n') {
            kept.push('\n');
        }
        if kept.len() > STDERR_LIMIT {
            let cut = kept.len() - STDERR_LIMIT;
            let boundary = (cut..kept.len())
                .find(|i| kept.is_char_boundary(*i))
                .unwrap_or(kept.len());
            kept.drain(..boundary);
        }
    }

    /// Transcript lines recorded since the previous call.
    pub(crate) fn take_transcript(&self) -> String {
        lock(&self.transcript).take_unread()
    }

    /// The whole transcript and the CLI's standard error, for the run.
    pub(crate) fn diagnostics(&self) -> Vec<(String, String)> {
        let mut files = Vec::new();
        let transcript = lock(&self.transcript);
        if !transcript.is_empty() {
            files.push(("cli-transcript.jsonl".to_owned(), transcript.render()));
        }
        let stderr = lock(&self.stderr);
        if !stderr.is_empty() {
            files.push(("cli-stderr.log".to_owned(), stderr.clone()));
        }
        files
    }
}

/// A child that printed nothing: exit 2 is the argument parser refusing
/// what Mortsom sent (a contract mismatch); any other failure is the CLI
/// crashing, a failure of the system under test.
fn without_document(command: &str, output: &ProcessOutput) -> Option<CliError> {
    if !output.stdout.trim().is_empty() {
        return None;
    }
    match output.code {
        Some(0) | None => None,
        Some(2) => Some(CliError::Contract {
            command: command.to_owned(),
            detail: "the CLI refused its arguments without an outcome document (exit 2)".into(),
        }),
        Some(exit) => Some(CliError::Crashed {
            command: command.to_owned(),
            exit,
        }),
    }
}

fn outcome(command: String, envelope: Envelope) -> Result<Envelope, CliError> {
    let (code, message, reason) = envelope
        .error
        .as_ref()
        .map(|e| (e.code.clone(), e.message.clone(), e.reason.clone()))
        .unwrap_or_default();
    match envelope.status {
        Status::Ok => Ok(envelope),
        Status::Error => Err(CliError::Refused {
            command,
            code,
            message,
            reason,
        }),
        Status::Uncertain => Err(CliError::Uncertain {
            command,
            code,
            message,
        }),
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}
