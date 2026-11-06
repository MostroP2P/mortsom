//! What one actor's invocations leave behind: the sanitized transcript and
//! the CLI's standard error. The task that runs an invocation holds a clone
//! and records the outcome itself, so a caller that gives up (a scenario
//! deadline) still leaves the command and its result in the evidence.

use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use mortsom_common::{Redactor, Secret};
use serde_json::Value;

use crate::call::Call;
use crate::envelope::{Envelope, ErrorInfo, Status};
use crate::error::CliError;
use crate::process::{ProcessFailed, ProcessOutput};
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
        result: Result<ProcessOutput, ProcessFailed>,
    ) -> Result<Envelope, CliError> {
        let command = call.name();
        let secrets: Vec<&Secret> = call.stdin().into_iter().collect();
        let output = match result {
            Ok(output) => output,
            Err(ProcessFailed { failure, stderr }) => {
                // A child that hung or was killed may have said why.
                self.keep_stderr(&stderr, &secrets);
                let what = Recorded::Failure(failure.to_string(), None);
                self.record(&command, argv, elapsed_ms, what);
                return Err(CliError::Process {
                    command,
                    cause: failure,
                });
            }
        };
        self.keep_stderr(&output.stderr, &secrets);
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
        let recorded = scrubbed(&envelope, &secrets);
        let what = Recorded::Outcome(&recorded, output.code);
        self.record(&command, argv, elapsed_ms, what);
        if envelope.command != command {
            let detail = scrub(&format!("answered as `{}`", envelope.command), &secrets);
            return Err(CliError::Contract { command, detail });
        }
        outcome(command, envelope, &secrets)
    }

    fn record(&self, command: &str, argv: &[String], elapsed_ms: u64, what: Recorded<'_>) {
        lock(&self.transcript).record(command, argv, elapsed_ms, what);
    }

    /// Keeps the CLI's standard error, free text from the system under
    /// test: scrubbed of invoices and of the invocation's secrets, then of
    /// anything key-shaped (nsec, mnemonic, NWC URI, long hex token).
    fn keep_stderr(&self, stderr: &str, secrets: &[&Secret]) {
        if stderr.is_empty() {
            return;
        }
        let stderr = Redactor::new().redact(&scrub(stderr, secrets));
        let mut kept = lock(&self.stderr);
        kept.push_str(&stderr);
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

/// The envelope as the transcript keeps it: every string and object key
/// the CLI printed (its error object, data, warnings and stages) loses the
/// invocation's secrets and every invoice. The transcript then replaces
/// whatever sits under a secret key on its own.
fn scrubbed(envelope: &Envelope, secrets: &[&Secret]) -> Envelope {
    Envelope {
        schema_version: envelope.schema_version.clone(),
        command: envelope.command.clone(),
        status: envelope.status,
        data: scrub_value(&envelope.data, secrets),
        error: envelope.error.as_ref().map(|error| ErrorInfo {
            code: scrub(&error.code, secrets),
            message: scrub(&error.message, secrets),
            reason: error.reason.as_deref().map(|reason| scrub(reason, secrets)),
        }),
        warnings: envelope
            .warnings
            .iter()
            .map(|warning| scrub_value(warning, secrets))
            .collect(),
        meta: scrub_value(&envelope.meta, secrets),
    }
}

/// `value` with every string and object key scrubbed, at any depth.
fn scrub_value(value: &Value, secrets: &[&Secret]) -> Value {
    match value {
        Value::String(text) => Value::String(scrub(text, secrets)),
        Value::Array(items) => Value::Array(
            items
                .iter()
                .map(|item| scrub_value(item, secrets))
                .collect(),
        ),
        Value::Object(map) => Value::Object(
            map.iter()
                .enumerate()
                .map(|(index, (key, inner))| {
                    (scrub_key(key, index, secrets), scrub_value(inner, secrets))
                })
                .collect(),
        ),
        other => other.clone(),
    }
}

/// An object key, scrubbed; a key that changed also carries `index`, its
/// position in the object, so two scrubbed keys never become one.
fn scrub_key(key: &str, index: usize, secrets: &[&Secret]) -> String {
    let scrubbed = scrub(key, secrets);
    if scrubbed == key {
        scrubbed
    } else {
        format!("{scrubbed}{index}")
    }
}

/// The `ok` outcome, or the failure the CLI reported. Its message and
/// reason reach the run's report, so they are scrubbed like any free text.
fn outcome(command: String, envelope: Envelope, secrets: &[&Secret]) -> Result<Envelope, CliError> {
    let (code, message, reason) = envelope
        .error
        .as_ref()
        .map(|e| {
            let reason = e.reason.as_deref().map(|reason| scrub(reason, secrets));
            (e.code.clone(), scrub(&e.message, secrets), reason)
        })
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

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    fn kept_stderr(evidence: &Evidence) -> String {
        evidence
            .diagnostics()
            .into_iter()
            .find(|(name, _)| name == "cli-stderr.log")
            .map(|(_, text)| text)
            .unwrap()
    }

    /// The cap keeps the newest standard error, and a cut that falls inside
    /// a multi-byte character moves past it instead of splitting it.
    #[test]
    fn standard_error_keeps_its_newest_part_cut_at_a_character_boundary() {
        let evidence = Evidence::default();
        // One byte, then a two-byte character whose second byte is exactly
        // where the cap cuts.
        let first = format!("x\u{e9}{}\n", ".".repeat(STDERR_LIMIT - 2));
        assert_eq!(first.len(), STDERR_LIMIT + 2);

        evidence.keep_stderr(&first, &[]);
        let cut = kept_stderr(&evidence);
        evidence.keep_stderr("newest", &[]);
        let kept = kept_stderr(&evidence);

        assert_eq!(cut, first[3..]);
        assert!(kept.len() <= STDERR_LIMIT, "{}", kept.len());
        assert!(kept.ends_with(".\nnewest\n"));
        assert!(!kept.contains('x') && !kept.contains('\u{e9}'));
    }
}
