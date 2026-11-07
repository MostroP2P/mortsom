//! What one actor's invocations leave behind: the sanitized transcript and
//! the CLI's standard error. The task that runs an invocation holds a clone
//! and records the outcome itself, so a caller that gives up (a scenario
//! deadline) still leaves the command and its result in the evidence.

use std::collections::BTreeSet;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use mortsom_common::{Redactor, Secret};
use serde_json::Value;

use crate::call::Call;
use crate::envelope::{Envelope, ErrorInfo, Status};
use crate::error::CliError;
use crate::process::{ProcessFailed, ProcessFailure, ProcessOutput};
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
                let cause = scrubbed_failure(failure, &secrets);
                let what = Recorded::Failure(cause.to_string(), None);
                self.record(&command, argv, elapsed_ms, what);
                return Err(CliError::Process { command, cause });
            }
        };
        self.keep_stderr(&output.stderr, &secrets);
        if let Some(error) = without_document(&command, &output) {
            let what = Recorded::Failure(scrub(&error.to_string(), &secrets), output.code);
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

/// The failure with its free text (a spawn or pipe error) scrubbed of the
/// invocation's secrets: it reaches the transcript and the run's report.
fn scrubbed_failure(failure: ProcessFailure, secrets: &[&Secret]) -> ProcessFailure {
    match failure {
        ProcessFailure::Spawn(text) => ProcessFailure::Spawn(scrub(&text, secrets)),
        ProcessFailure::Io(text) => ProcessFailure::Io(scrub(&text, secrets)),
        // Listed, so that a new variant carrying text must say how it is
        // scrubbed.
        failure @ (ProcessFailure::Timeout(_) | ProcessFailure::Signaled) => failure,
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
        Value::Object(map) => {
            let mut taken: BTreeSet<String> = map.keys().cloned().collect();
            Value::Object(
                map.iter()
                    .enumerate()
                    .map(|(index, (key, inner))| {
                        let key = scrub_key(key, index, secrets, &mut taken);
                        (key, scrub_value(inner, secrets))
                    })
                    .collect(),
            )
        }
        other => other.clone(),
    }
}

/// An object key, scrubbed. A key that changed also carries a number,
/// counting up from `index`, its position in the object, until the name is
/// in `taken` neither as a key the CLI wrote nor as one already given out.
fn scrub_key(key: &str, index: usize, secrets: &[&Secret], taken: &mut BTreeSet<String>) -> String {
    let scrubbed = scrub(key, secrets);
    if scrubbed == key {
        return scrubbed;
    }
    let name = (index..)
        .map(|n| format!("{scrubbed}{n}"))
        .find(|name| !taken.contains(name))
        .unwrap_or(scrubbed);
    taken.insert(name.clone());
    name
}

/// The `ok` outcome, or the failure the CLI reported. Its code, message
/// and reason reach the run's report, so they are scrubbed like any free
/// text; a code that held a secret is no documented code anyway.
fn outcome(command: String, envelope: Envelope, secrets: &[&Secret]) -> Result<Envelope, CliError> {
    let (code, message, reason) = envelope
        .error
        .as_ref()
        .map(|e| {
            let reason = e.reason.as_deref().map(|reason| scrub(reason, secrets));
            (scrub(&e.code, secrets), scrub(&e.message, secrets), reason)
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

    #[test]
    fn a_scrubbed_key_never_replaces_a_key_the_cli_wrote() {
        let secret = Secret::new("someone@example.com");
        let value = serde_json::json!({"[redacted]1": "kept", "someone@example.com": "x"});

        let scrubbed = scrub_value(&value, &[&secret]);

        assert_eq!(scrubbed.as_object().map(|m| m.len()), Some(2), "{scrubbed}");
        assert_eq!(scrubbed["[redacted]1"], "kept");
        assert!(
            !scrubbed.to_string().contains("someone@example.com"),
            "{scrubbed}"
        );
    }

    fn add_invoice(address: &Secret) -> (Call, Vec<String>) {
        let call = Call::node(&["order", "add-invoice"]).invoice(address);
        let argv = call.argv(&crate::call::NodeTarget {
            mostro_pubkey: "ab".repeat(32),
            relays: vec!["ws://127.0.0.1:7500".into()],
            reply_timeout_secs: None,
        });
        (call, argv)
    }

    fn kept_transcript(evidence: &Evidence) -> String {
        evidence
            .diagnostics()
            .into_iter()
            .find(|(name, _)| name == "cli-transcript.jsonl")
            .map(|(_, text)| text)
            .unwrap()
    }

    /// A process failure's text is free text too: a Lightning address on
    /// standard input matches no invoice pattern, so only the invocation's
    /// own secrets keep it out of the transcript and the error, whichever
    /// failure carries it.
    #[test]
    fn a_process_failure_loses_the_invocation_s_secret() {
        let address = Secret::new("someone@example.com");
        let (call, argv) = add_invoice(&address);
        for failure in [
            ProcessFailure::Spawn("cannot run Someone@Example.com: broken pipe".into()),
            ProcessFailure::Io("cannot write Someone@Example.com: broken pipe".into()),
        ] {
            let evidence = Evidence::default();

            let error = evidence
                .interpret(&call, &argv, 1, Err(failure.into()))
                .unwrap_err();

            let read = evidence.take_transcript();
            let kept = kept_transcript(&evidence);
            for text in [&read, &kept, &error.to_string()] {
                assert!(text.contains("broken pipe"), "{text}");
                assert!(
                    !text.to_lowercase().contains("someone@example.com"),
                    "{text}"
                );
            }
        }
    }

    /// The error's code reaches the run's report like its message, so a
    /// secret the CLI echoed into it is scrubbed there too.
    #[test]
    fn a_refusal_s_code_loses_the_invocation_s_secret() {
        let address = Secret::new("someone@example.com");
        let (call, argv) = add_invoice(&address);
        for status in ["error", "uncertain"] {
            let stdout = format!(
                r#"{{"schema_version":"1","command":"order.add-invoice","status":"{status}",
                "error":{{"code":"bad_someone@example.com","message":"refused"}}}}"#
            );
            let output = ProcessOutput {
                code: Some(1),
                stdout,
                stderr: String::new(),
            };

            let error = Evidence::default()
                .interpret(&call, &argv, 1, Ok(output))
                .unwrap_err();

            let code = error.code().unwrap_or_default();
            assert!(code.starts_with("bad_"), "{code}");
            for text in [code, &error.to_string()] {
                assert!(!text.contains("someone@example.com"), "{text}");
            }
        }
    }

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
