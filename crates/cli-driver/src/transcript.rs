//! The sanitized record of every invocation an actor made: what a screen
//! capture is for an app, the transcript is for the CLI (spec §12). Invoices
//! and payment requests never enter it; the standard input never does.

use std::ops::Range;

use serde::Serialize;
use serde_json::Value;

use crate::envelope::{Envelope, ErrorInfo};

/// Keys whose whole value is a payment secret: every string and object in
/// it, at any depth, is replaced.
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
    /// Records one invocation. `argv` is sanitized here, again, and every
    /// free-text field (the error object, the failure) loses its invoices.
    pub fn record(&mut self, command: &str, argv: &[String], elapsed_ms: u64, what: Recorded<'_>) {
        let (exit, status, error, failure, data) = match what {
            Recorded::Outcome(envelope, exit) => {
                let mut data = envelope.data.clone();
                redact_value(&mut data);
                (
                    exit,
                    Some(envelope.status.as_str()),
                    envelope.error.as_ref().map(scrub_error),
                    None,
                    data,
                )
            }
            Recorded::Failure(reason, exit) => {
                (exit, None, None, Some(scrub(&reason, &[])), Value::Null)
            }
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

/// The error object with every Lightning invoice in its text replaced: the
/// CLI under test fills it, so nothing in it is trusted to be clean.
fn scrub_error(error: &ErrorInfo) -> ErrorInfo {
    ErrorInfo {
        code: scrub(&error.code, &[]),
        message: scrub(&error.message, &[]),
        reason: error.reason.as_deref().map(|reason| scrub(reason, &[])),
    }
}

/// Replaces the value of `--invoice`, given as `--invoice <value>` or
/// `--invoice=<value>`, unless it is `-` (stdin), and every Lightning
/// invoice in any argument.
pub fn sanitize_argv(argv: &[String]) -> Vec<String> {
    let mut out = Vec::with_capacity(argv.len());
    let mut secret_next = false;
    for arg in argv {
        out.push(sanitize_arg(arg, secret_next));
        secret_next = arg == "--invoice";
    }
    out
}

/// One argument, sanitized; `secret_next` says it follows `--invoice`.
fn sanitize_arg(arg: &str, secret_next: bool) -> String {
    if secret_next && arg != "-" {
        return REDACTED.to_owned();
    }
    match arg.strip_prefix("--invoice=") {
        Some(value) if value != "-" => format!("--invoice={REDACTED}"),
        _ => scrub(arg, &[]),
    }
}

/// Replaces everything under a secret key (any key naming an invoice
/// included, see [`redact_strings`]) and every string or object key that
/// is a Lightning invoice, at any depth. Elsewhere nulls, numbers and
/// booleans stay, so the shape stays readable.
pub fn redact_value(value: &mut Value) {
    match value {
        Value::Object(map) => {
            for (index, (key, mut inner)) in std::mem::take(map).into_iter().enumerate() {
                if is_secret_key(&key) {
                    redact_strings(&mut inner);
                } else {
                    redact_value(&mut inner);
                }
                map.insert(redact_key(key, index), inner);
            }
        }
        Value::Array(items) => items.iter_mut().for_each(redact_value),
        Value::String(text) if lightning().is_some_and(|re| re.is_match(text)) => {
            *text = REDACTED.into();
        }
        _ => {}
    }
}

/// The whole value of a secret key: every string at any depth is replaced,
/// and so is every object, whose keys may be secrets too. Nulls, numbers
/// and booleans stay.
fn redact_strings(value: &mut Value) {
    match value {
        Value::String(_) | Value::Object(_) => *value = Value::String(REDACTED.into()),
        Value::Array(items) => items.iter_mut().for_each(redact_strings),
        _ => {}
    }
}

/// An object key, replaced when it holds a Lightning invoice; `index`, its
/// position in the object, keeps replaced keys distinct.
fn redact_key(key: String, index: usize) -> String {
    if lightning().is_some_and(|re| re.is_match(&key)) {
        format!("{REDACTED}{index}")
    } else {
        key
    }
}

fn is_secret_key(key: &str) -> bool {
    SECRET_KEYS.contains(&key) || key.contains("invoice")
}

/// Lightning invoices and LNURLs, whatever the network, wherever they start.
///
/// There is no word boundary before the prefix: a letter, digit, `_` or the
/// tail of an escape such as `\n` may sit right before an invoice, and the
/// only cost of matching there is over-redaction.
fn lightning() -> Option<&'static regex::Regex> {
    static PATTERN: std::sync::LazyLock<Option<regex::Regex>> = std::sync::LazyLock::new(|| {
        regex::Regex::new(r"(?i)(?:ln(?:bcrt|bc|tbs|tb|sb)[0-9a-z]{20,}|lnurl[0-9a-z]{20,})").ok()
    });
    PATTERN.as_ref()
}

/// Scrubs free text (standard error, a parser's complaint): every Lightning
/// invoice, and every given secret, is replaced.
///
/// Every match is found in the original text first and overlapping matches
/// are merged, so no replacement can split another match and leave part of
/// a secret behind.
pub fn scrub(text: &str, secrets: &[&mortsom_common::Secret]) -> String {
    let mut ranges: Vec<Range<usize>> = lightning()
        .map(|re| re.find_iter(text).map(|m| m.range()).collect())
        .unwrap_or_default();
    for value in secrets.iter().map(|s| s.expose()).filter(|v| !v.is_empty()) {
        ranges.extend(occurrences(text, value));
    }
    replace_ranges(text, merge(ranges))
}

/// Every occurrence of `needle` in `text`, overlapping ones included.
fn occurrences(text: &str, needle: &str) -> Vec<Range<usize>> {
    let mut found = Vec::new();
    let mut from = 0;
    while let Some(at) = text[from..].find(needle) {
        let start = from + at;
        found.push(start..start + needle.len());
        from = start + text[start..].chars().next().map_or(1, char::len_utf8);
    }
    found
}

/// The ranges sorted, with overlapping or touching ones joined.
fn merge(mut ranges: Vec<Range<usize>>) -> Vec<Range<usize>> {
    ranges.sort_by_key(|r| r.start);
    let mut merged: Vec<Range<usize>> = Vec::with_capacity(ranges.len());
    for range in ranges {
        match merged.last_mut() {
            Some(last) if range.start <= last.end => last.end = last.end.max(range.end),
            _ => merged.push(range),
        }
    }
    merged
}

/// `text` with each of the sorted, disjoint `ranges` replaced once.
fn replace_ranges(text: &str, ranges: Vec<Range<usize>>) -> String {
    let mut out = String::with_capacity(text.len());
    let mut cursor = 0;
    for range in ranges {
        out.push_str(&text[cursor..range.start]);
        out.push_str(REDACTED);
        cursor = range.end;
    }
    out.push_str(&text[cursor..]);
    out
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
    fn failure_text_and_error_fields_never_carry_an_invoice() {
        let invoice = "lnbcrt10n1pqqqqqqqqqqqqqqqqqqqqqqqqqqq";
        let refusal = Envelope::parse(
            &format!(
                r#"{{"schema_version":"1","command":"order.add-invoice","status":"error",
                "data":null,"error":{{"code":"node_refused","message":"bad {invoice}",
                "reason":"{invoice}"}},"warnings":[],"meta":{{}}}}"#
            ),
            Some(1),
        )
        .unwrap();
        let complaint = format!("outcome schema version `{invoice}` is not the supported `1`");
        let mut transcript = Transcript::default();

        transcript.record("order.info", &[], 1, Recorded::Failure(complaint, Some(0)));
        transcript.record(
            "order.add-invoice",
            &[],
            2,
            Recorded::Outcome(&refusal, Some(1)),
        );

        let text = transcript.render();
        assert!(!text.contains("lnbcrt1"), "{text}");
        assert!(text.contains("\"code\":\"node_refused\""), "{text}");
        assert!(
            text.contains(&format!("\"message\":\"bad {REDACTED}\"")),
            "{text}"
        );
        assert!(
            text.contains(&format!("\"reason\":\"{REDACTED}\"")),
            "{text}"
        );
        assert!(
            text.contains(&format!("schema version `{REDACTED}`")),
            "{text}"
        );
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
    fn strings_nested_under_a_secret_key_are_redacted() {
        let mut value = serde_json::json!({
            "payment_request": [null, "user@example.com", 150],
            "buyer_invoice": [["someone@example.com", false]],
            "order_id": "o1",
        });

        redact_value(&mut value);

        assert_eq!(
            value["payment_request"],
            serde_json::json!([null, REDACTED, 150])
        );
        assert_eq!(
            value["buyer_invoice"],
            serde_json::json!([[REDACTED, false]])
        );
        assert_eq!(value["order_id"], "o1");
    }

    #[test]
    fn an_invoice_used_as_an_object_key_is_redacted() {
        let first = format!("lnbcrt10n1p{}", "q".repeat(40));
        let second = format!("lnbcrt10n1p{}", "p".repeat(40));
        let mut value: Value = serde_json::from_str(&format!(
            r#"{{"orders":{{"{first}":"paid","{second}":"open","o1":"active"}},
            "invoice":{{"{first}":1}}}}"#
        ))
        .unwrap();

        redact_value(&mut value);

        let text = value.to_string();
        assert!(!text.contains("lnbcrt10n1p"), "{text}");
        assert_eq!(value["orders"].as_object().map(|m| m.len()), Some(3));
        assert_eq!(value["orders"]["o1"], "active");
    }

    #[test]
    fn an_object_under_a_secret_key_is_redacted_whole() {
        let mut value = serde_json::json!({
            "payment_request": {"user@example.com": null},
            "buyer_invoice": [null, {"address": "someone@example.com", "paid": false}],
        });

        redact_value(&mut value);

        assert_eq!(value["payment_request"], REDACTED);
        assert_eq!(value["buyer_invoice"], serde_json::json!([null, REDACTED]));
    }

    #[test]
    fn a_payment_request_payload_never_reaches_the_transcript() {
        let envelope = outcome(r#"{"payment_request":[null,"user@example.com",null]}"#);
        let mut transcript = Transcript::default();

        transcript.record(
            "order.take-buy",
            &[],
            3,
            Recorded::Outcome(&envelope, Some(0)),
        );

        let text = transcript.render();
        assert!(!text.contains("user@example.com"), "{text}");
    }

    #[test]
    fn free_text_loses_invoices_and_the_given_secrets() {
        let secret = mortsom_common::Secret::new("alice@example.com");
        let text = "debug: paying lnbcrt10n1pqqqqqqqqqqqqqqqqqqqqqqqqqqq to alice@example.com";

        let scrubbed = scrub(text, &[&secret]);

        assert_eq!(scrubbed, "debug: paying [redacted] to [redacted]");
    }

    #[test]
    fn an_invoice_glued_to_a_word_character_is_still_redacted() {
        let invoice = format!("lnbcrt10n1p{}", "q".repeat(40));
        let complaint = Envelope::parse(
            &format!(
                r#"{{"schema_version":"1","command":"c","status":"ok","warnings":"\n{invoice}"}}"#
            ),
            Some(0),
        )
        .unwrap_err();
        let texts = [
            complaint,
            format!("x_{invoice}"),
            format!("%3A{invoice}"),
            format!("日本{invoice}"),
        ];

        for text in texts {
            let scrubbed = scrub(&text, &[]);
            assert!(!scrubbed.contains("lnbcrt10n1p"), "{scrubbed}");
        }
        let argv = vec!["--note".to_owned(), format!("x_{invoice}")];
        assert_eq!(sanitize_argv(&argv), ["--note", "x_[redacted]"]);
    }

    #[test]
    fn a_secret_wrapping_an_invoice_is_redacted_whole() {
        let value = format!("lnbcrt1{}-tail-secret", "q".repeat(24));
        let secret = mortsom_common::Secret::new(value.clone());

        let scrubbed = scrub(&format!("err: {value} end"), &[&secret]);

        assert_eq!(scrubbed, "err: [redacted] end");
    }

    #[test]
    fn a_shorter_secret_never_splits_a_longer_one() {
        let short = mortsom_common::Secret::new("key");
        let long = mortsom_common::Secret::new("monkeyboy");

        let scrubbed = scrub("x monkeyboy y", &[&short, &long]);

        assert_eq!(scrubbed, "x [redacted] y");
    }

    #[test]
    fn partially_overlapping_secrets_are_both_removed() {
        let left = mortsom_common::Secret::new("abcd");
        let right = mortsom_common::Secret::new("cdef");

        assert_eq!(scrub("abcdef!", &[&left, &right]), "[redacted]!");
    }

    #[test]
    fn a_secret_overlapping_itself_leaves_nothing_behind() {
        let secret = mortsom_common::Secret::new("aba");

        assert_eq!(scrub("x ababa y", &[&secret]), "x [redacted] y");
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
    fn an_equals_form_invoice_is_masked_but_its_stdin_value_is_named() {
        let argv: Vec<String> = ["--invoice=someone@example.com", "--invoice=-"]
            .iter()
            .map(|s| (*s).to_owned())
            .collect();

        assert_eq!(
            sanitize_argv(&argv),
            [format!("--invoice={REDACTED}"), "--invoice=-".to_owned()]
        );
    }

    #[test]
    fn an_invoice_under_any_flag_is_masked() {
        let invoice = format!("lnbcrt1{}", "q".repeat(24));
        let argv = vec![
            "--note".to_owned(),
            invoice,
            "--order".to_owned(),
            "o1".to_owned(),
        ];

        assert_eq!(sanitize_argv(&argv), ["--note", REDACTED, "--order", "o1"]);
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
