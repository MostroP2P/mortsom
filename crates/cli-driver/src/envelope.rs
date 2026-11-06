//! The machine-interface envelope of mostro-cli (contract version 1): every
//! invocation with `--output json` prints exactly one document on stdout,
//! whatever its outcome.

use serde::Deserialize;
use serde_json::Value;

/// The contract version Mortsom speaks.
pub const SCHEMA_VERSION: &str = "1";

/// How a command ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    /// The command did what it was asked.
    Ok,
    /// The command provably did nothing on the node, or the node refused.
    Error,
    /// A request was published and its outcome is unknown.
    Uncertain,
}

impl Status {
    /// Wire name.
    pub fn as_str(self) -> &'static str {
        match self {
            Status::Ok => "ok",
            Status::Error => "error",
            Status::Uncertain => "uncertain",
        }
    }
}

/// The error object of a failed command: a stable code and fixed prose.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, serde::Serialize)]
pub struct ErrorInfo {
    /// Stable snake_case code (`node_refused`, `invalid_arguments`, ...).
    pub code: String,
    /// Fixed prose chosen by the code; never caller-provided text.
    pub message: String,
    /// The node's own reason for a refusal (`order_already_canceled`,
    /// `invalid_invoice`, ...), when it gave one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// One parsed document.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Envelope {
    /// Contract version; must be [`SCHEMA_VERSION`].
    pub schema_version: String,
    /// Dotted command name (`order.take-sell`).
    pub command: String,
    /// Outcome.
    pub status: Status,
    /// Command-specific data; `null` on most failures.
    #[serde(default)]
    pub data: Value,
    /// Present when the status is not `ok`.
    #[serde(default)]
    pub error: Option<ErrorInfo>,
    /// Warnings attached to an outcome.
    #[serde(default)]
    pub warnings: Vec<Value>,
    /// Execution stages (publication, daemon acceptance, persistence).
    #[serde(default)]
    pub meta: Value,
}

impl Envelope {
    /// Parses the single document on stdout and checks that the exit code
    /// agrees with its status (0 for `ok`, 1 or 2 otherwise).
    pub fn parse(stdout: &str, exit: Option<i32>) -> Result<Envelope, String> {
        let text = stdout.trim();
        if text.is_empty() {
            return Err("the CLI printed no JSON document".into());
        }
        let envelope: Envelope = serde_json::from_str(text)
            .map_err(|e| format!("the CLI printed no valid outcome document ({e})"))?;
        if envelope.schema_version != SCHEMA_VERSION {
            return Err(format!(
                "outcome schema version `{}` is not the supported `{SCHEMA_VERSION}`",
                envelope.schema_version
            ));
        }
        let consistent = match envelope.status {
            Status::Ok => exit == Some(0),
            Status::Error | Status::Uncertain => matches!(exit, Some(1 | 2)),
        };
        if !consistent {
            return Err(format!(
                "exit code {exit:?} contradicts outcome status `{}`",
                envelope.status.as_str()
            ));
        }
        if envelope.status != Status::Ok && envelope.error.is_none() {
            return Err("a failed outcome carries no error code".into());
        }
        Ok(envelope)
    }

    /// The value at `path` inside `data`, if any and not `null`.
    pub fn field(&self, path: &[&str]) -> Option<&Value> {
        let mut value = &self.data;
        for key in path {
            value = value.get(key)?;
        }
        (!value.is_null()).then_some(value)
    }

    /// The string at `path` inside `data`.
    pub fn text(&self, path: &[&str]) -> Option<&str> {
        self.field(path).and_then(Value::as_str)
    }

    /// An unsigned integer at `path` inside `data`. The contract writes
    /// amounts as decimal strings so no JSON number loses precision; a
    /// plain number is accepted too.
    pub fn unsigned(&self, path: &[&str]) -> Option<u64> {
        unsigned_of(self.field(path)?)
    }
}

/// An unsigned integer written either as a decimal string or a number.
pub fn unsigned_of(value: &Value) -> Option<u64> {
    match value {
        Value::String(text) => text.parse().ok(),
        Value::Number(number) => number.as_u64(),
        _ => None,
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    const OK: &str = r#"{"schema_version":"1","command":"order.info","status":"ok",
        "data":{"orders":[{"id":"o1","status":"active","sats_amount":"15000"}],"none":null},
        "error":null,"warnings":[],"meta":{}}"#;

    #[test]
    fn a_success_document_is_read_with_its_data() {
        let envelope = Envelope::parse(OK, Some(0)).unwrap();

        assert_eq!(envelope.command, "order.info");
        assert_eq!(envelope.status, Status::Ok);
        let order = &envelope.field(&["orders"]).unwrap()[0];
        assert_eq!(order["status"], "active");
        assert_eq!(unsigned_of(&order["sats_amount"]), Some(15000));
        assert!(envelope.field(&["none"]).is_none());
        assert!(envelope.field(&["missing", "deeper"]).is_none());
    }

    #[test]
    fn a_refusal_carries_its_code() {
        let text = r#"{"schema_version":"1","command":"order.take-sell","status":"error",
            "data":null,"error":{"code":"node_refused","message":"The node refused."},
            "warnings":[],"meta":{}}"#;

        let envelope = Envelope::parse(text, Some(1)).unwrap();

        assert_eq!(envelope.error.unwrap().code, "node_refused");
    }

    #[test]
    fn an_exit_code_that_contradicts_the_status_breaks_the_contract() {
        assert!(Envelope::parse(OK, Some(1)).is_err());
        assert!(Envelope::parse(OK, None).is_err());
    }

    #[test]
    fn anything_but_one_versioned_document_breaks_the_contract() {
        assert!(Envelope::parse("", Some(0)).is_err());
        assert!(Envelope::parse("Order created!", Some(0)).is_err());
        let future = OK.replace(r#""schema_version":"1""#, r#""schema_version":"2""#);
        assert!(Envelope::parse(&future, Some(0)).is_err());
    }

    #[test]
    fn a_failure_without_an_error_code_breaks_the_contract() {
        let text = r#"{"schema_version":"1","command":"order.new","status":"uncertain",
            "data":null,"error":null,"warnings":[],"meta":{}}"#;
        assert!(Envelope::parse(text, Some(1)).is_err());
    }
}
