//! The sanitized record of every invocation an actor made: what a screen
//! capture is for an app, the transcript is for the CLI (spec §12). Invoices
//! and payment requests never enter it; the standard input never does.

use serde::Serialize;
use serde_json::Value;

use crate::envelope::{Envelope, ErrorInfo};

/// Keys whose string values are payment secrets and are replaced.
const SECRET_KEYS: [&str; 4] = ["invoice", "payment_request", "bolt11", "lnurl"];
/// What a replaced value reads.
pub const REDACTED: &str = "[redacted]";

/// One invocation as recorded.
#[derive(Debug, Clone, Serialize)]
struct Entry {
    seq: usize,
    command: String,
    argv: Vec<String>,
    exit: Option<i32>,
    status: Option<&'static str>,
    error: Option<ErrorInfo>,
    failure: Option<String>,
    elapsed_ms: u64,
    data: Value,
}

/// What an invocation produced, for the record.
pub enum Recorded<'a> {
    /// A parsed outcome document.
    Outcome(&'a Envelope, Option<i32>),
    /// No usable document: why.
    Failure(String, Option<i32>),
}

/// An append-only list of JSON lines.
#[derive(Debug, Default)]
pub struct Transcript {
    lines: Vec<String>,
    read: usize,
}

impl Transcript {
    /// Records one invocation. `argv` is sanitized here, again.
    pub fn record(&mut self, command: &str, argv: &[String], elapsed_ms: u64, what: Recorded<'_>) {
        let (exit, status, error, failure, data) = match what {
            Recorded::Outcome(envelope, exit) => {
                let mut data = envelope.data.clone();
                redact_value(&mut data);
                (
                    exit,
                    Some(envelope.status.as_str()),
                    envelope.error.clone(),
                    None,
                    data,
                )
            }
            Recorded::Failure(reason, exit) => (exit, None, None, Some(reason), Value::Null),
        };
        let entry = Entry {
            seq: self.lines.len() + 1,
            command: command.to_owned(),
            argv: sanitize_argv(argv),
            exit,
            status,
            error,
            failure,
            elapsed_ms,
            data,
        };
        let line = serde_json::to_string(&entry)
            .unwrap_or_else(|_| format!("{{\"seq\":{},\"unrecordable\":true}}", entry.seq));
        self.lines.push(line);
    }

    /// Lines recorded since the previous call, one JSON document per line.
    pub fn take_unread(&mut self) -> String {
        let unread = self.lines[self.read..].join("\n");
        self.read = self.lines.len();
        unread
    }

    /// Every line recorded.
    pub fn render(&self) -> String {
        self.lines.join("\n")
    }

    /// How many invocations were recorded.
    pub fn len(&self) -> usize {
        self.lines.len()
    }

    /// Whether nothing was recorded.
    pub fn is_empty(&self) -> bool {
        self.lines.is_empty()
    }
}

/// Replaces the value following `--invoice` unless it is `-` (stdin).
pub fn sanitize_argv(argv: &[String]) -> Vec<String> {
    let mut out = Vec::with_capacity(argv.len());
    let mut secret_next = false;
    for arg in argv {
        if secret_next && arg != "-" {
            out.push(REDACTED.to_owned());
        } else {
            out.push(arg.clone());
        }
        secret_next = arg == "--invoice";
    }
    out
}

/// Replaces every string under a secret key (any key naming an invoice
/// included) and every string that is a Lightning invoice, at any depth.
pub fn redact_value(value: &mut Value) {
    match value {
        Value::Object(map) => {
            for (key, inner) in map.iter_mut() {
                if is_secret_key(key) && inner.is_string() {
                    *inner = Value::String(REDACTED.into());
                } else {
                    redact_value(inner);
                }
            }
        }
        Value::Array(items) => items.iter_mut().for_each(redact_value),
        Value::String(text) if lightning().is_some_and(|re| re.is_match(text)) => {
            *text = REDACTED.into();
        }
        _ => {}
    }
}

fn is_secret_key(key: &str) -> bool {
    SECRET_KEYS.contains(&key) || key.contains("invoice")
}

/// Lightning invoices and LNURLs, whatever the network.
fn lightning() -> Option<&'static regex::Regex> {
    static PATTERN: std::sync::LazyLock<Option<regex::Regex>> = std::sync::LazyLock::new(|| {
        regex::Regex::new(r"(?i)\b(?:ln(?:bcrt|bc|tbs|tb|sb)[0-9a-z]{20,}|lnurl[0-9a-z]{20,})").ok()
    });
    PATTERN.as_ref()
}

/// Scrubs free text (standard error, a parser's complaint): every Lightning
/// invoice, and every given secret, is replaced.
pub fn scrub(text: &str, secrets: &[&mortsom_common::Secret]) -> String {
    let mut scrubbed = match lightning() {
        Some(re) => re.replace_all(text, REDACTED).into_owned(),
        None => text.to_owned(),
    };
    for secret in secrets {
        if !secret.expose().is_empty() {
            scrubbed = scrubbed.replace(secret.expose(), REDACTED);
        }
    }
    scrubbed
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    fn outcome(data: &str) -> Envelope {
        Envelope::parse(
            &format!(
                r#"{{"schema_version":"1","command":"order.take-buy","status":"ok",
                "data":{data},"error":null,"warnings":[],"meta":{{}}}}"#
            ),
            Some(0),
        )
        .unwrap()
    }

    #[test]
    fn payment_requests_never_reach_the_transcript() {
        let envelope = outcome(
            r#"{"payment_request":"lnbcrt1hold","messages":[{"payload":{"invoice":"lnbcrt1bond"}}],"order_id":"o1"}"#,
        );
        let mut transcript = Transcript::default();

        transcript.record(
            "order.take-buy",
            &["--order".into(), "o1".into()],
            12,
            Recorded::Outcome(&envelope, Some(0)),
        );

        let text = transcript.render();
        assert!(!text.contains("lnbcrt1"), "{text}");
        assert!(text.contains("\"order_id\":\"o1\""), "{text}");
        assert!(text.contains(REDACTED));
    }

    #[test]
    fn invoices_are_redacted_by_value_and_by_any_key_naming_one() {
        let mut value = serde_json::json!({
            "order": {"buyer_invoice": "someone@example.com", "status": "active"},
            "note": "LNBCRT10N1PNEWINVOICEXXXXXXXXXXXXXXXXXXX",
            "list": ["lntbs1qqqqqqqqqqqqqqqqqqqqqqqqqqqq", "plain"],
        });

        redact_value(&mut value);

        assert_eq!(value["order"]["buyer_invoice"], REDACTED);
        assert_eq!(value["order"]["status"], "active");
        assert_eq!(value["note"], REDACTED);
        assert_eq!(value["list"][0], REDACTED);
        assert_eq!(value["list"][1], "plain");
    }

    #[test]
    fn free_text_loses_invoices_and_the_given_secrets() {
        let secret = mortsom_common::Secret::new("alice@example.com");
        let text = "debug: paying lnbcrt10n1pqqqqqqqqqqqqqqqqqqqqqqqqqqq to alice@example.com";

        let scrubbed = scrub(text, &[&secret]);

        assert_eq!(scrubbed, "debug: paying [redacted] to [redacted]");
    }

    #[test]
    fn a_literal_invoice_argument_is_masked_but_stdin_is_named() {
        let argv: Vec<String> = ["--invoice", "lnbcrt1x", "--invoice", "-"]
            .iter()
            .map(|s| (*s).to_owned())
            .collect();

        assert_eq!(
            sanitize_argv(&argv),
            ["--invoice", REDACTED, "--invoice", "-"]
        );
    }

    #[test]
    fn unread_lines_are_handed_out_once_and_the_whole_stays_available() {
        let mut transcript = Transcript::default();
        let ok = outcome(r#"{"order_id":"o1"}"#);
        transcript.record("order.take-buy", &[], 1, Recorded::Outcome(&ok, Some(0)));
        transcript.record(
            "order.info",
            &[],
            2,
            Recorded::Failure("timed out".into(), None),
        );

        let first = transcript.take_unread();
        let second = transcript.take_unread();

        assert_eq!(first.lines().count(), 2);
        assert!(second.is_empty());
        assert_eq!(transcript.render().lines().count(), 2);
        assert_eq!(transcript.len(), 2);
        assert!(first.contains("\"failure\":\"timed out\""), "{first}");
    }
}
