#![allow(clippy::unwrap_used, clippy::expect_used)]

use super::*;
use mortsom_cli_driver::TokioCliProcess;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;

fn discovery_document(commands: Value) -> String {
    serde_json::json!({
        "schema_version": "1",
        "command": "schema",
        "status": "ok",
        "data": {
            "schema_version": "1", "executable_version": "0.17.0", "commands": commands,
            "global_options": [{"name": "output"}, {"name": "reply-timeout", "default": "15"}],
        },
        "error": null,
        "warnings": [],
        "meta": {},
    })
    .to_string()
}

fn json_command(name: &str) -> Value {
    serde_json::json!({"name": name, "output_modes": ["human", "json"], "input_schema": null})
}

fn every_required() -> Vec<Value> {
    REQUIRED_COMMANDS
        .iter()
        .map(|c| match *c {
            "order.add-invoice" => serde_json::json!({
                "name": c, "output_modes": ["json"],
                "input_schema": {"properties": {"invoice": {"type": "string", "x-stdin": "-"}}}
            }),
            _ => json_command(c),
        })
        .collect()
}

#[test]
fn a_cli_that_takes_the_invoice_only_as_an_argument_is_refused() {
    let commands: Vec<Value> = REQUIRED_COMMANDS.iter().map(|c| json_command(c)).collect();

    let discovery = Discovery::parse(&discovery_document(Value::Array(commands)), Some(0)).unwrap();
    let error = require(&discovery).unwrap_err();

    assert!(!discovery.stdin_invoice);
    assert!(error.contains("standard input"), "{error}");
}

/// The contract's sentinel is `"x-stdin": "-"`: any other value is a CLI
/// that would refuse `--invoice -` only once a trade is under way.
#[test]
fn a_cli_whose_stdin_marker_is_not_the_dash_is_refused() {
    for marker in [r#"null"#, r#"false"#, r#""yes""#] {
        let text = discovery_document(Value::Array(every_required()))
            .replace(r#""x-stdin":"-""#, &format!(r#""x-stdin":{marker}"#));
        assert!(text.contains(&format!(r#""x-stdin":{marker}"#)), "{text}");

        let discovery = Discovery::parse(&text, Some(0)).unwrap();

        assert!(!discovery.stdin_invoice, "{marker}");
        assert!(require(&discovery).is_err(), "{marker}");
    }
}

#[test]
fn a_cli_without_a_reply_budget_keeps_its_default() {
    let text = discovery_document(Value::Array(every_required())).replace(
        r#"{"default":"15","name":"reply-timeout"}"#,
        r#"{"name":"verbose"}"#,
    );

    let discovery = Discovery::parse(&text, Some(0)).unwrap();

    assert!(!discovery.reply_timeout);
}

/// A shell script standing in for the executable: it prints `stdout`.
fn fake_cli(dir: &Path, stdout: &str) -> PathBuf {
    let path = dir.join("fake-mostro-cli");
    std::fs::write(&path, format!("#!/bin/sh\ncat <<'EOF'\n{stdout}\nEOF\n")).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    path
}

#[test]
fn the_profile_decides_the_runtime_and_refuses_a_platform_with_the_cli() {
    assert_eq!(
        resolve_client("mobile-v2", None, None),
        Ok(("mobile-v2".into(), Platform::Linux))
    );
    assert_eq!(
        resolve_client("mobile-v2", None, Some(Platform::Web)),
        Ok(("mobile-v2".into(), Platform::Web))
    );
    assert_eq!(
        resolve_client("mobile-v2", Some("cli"), None),
        Ok(("cli".into(), Platform::Cli))
    );
    assert_eq!(
        resolve_client("cli", None, None),
        Ok(("cli".into(), Platform::Cli))
    );
    assert!(resolve_client("mobile-v2", Some("cli"), Some(Platform::Linux)).is_err());
    assert!(resolve_client("mobile-v2", Some("android"), None).is_err());
}

#[test]
fn discovery_keeps_only_commands_with_a_json_mode() {
    let mut commands = every_required();
    commands
        .push(serde_json::json!({"name": "account.conversation-key", "output_modes": ["human"]}));
    commands.push(serde_json::json!({
        "name": "order.new", "output_modes": ["json"],
        "input_schema": {"properties": {"expires-in": {"type": "string"}}}
    }));

    let discovery = Discovery::parse(&discovery_document(Value::Array(commands)), Some(0)).unwrap();

    assert_eq!(discovery.version, "0.17.0");
    assert!(discovery.commands.contains("order.new"));
    assert!(!discovery.commands.contains("account.conversation-key"));
    assert!(discovery.order_expiry_in_seconds);
    assert!(require(&discovery).is_ok());
}

#[test]
fn a_cli_without_the_identity_commands_is_refused_by_name() {
    let commands: Vec<Value> = every_required()
        .into_iter()
        .filter(|c| !c["name"].as_str().unwrap().starts_with("identity."))
        .collect();

    let discovery = Discovery::parse(&discovery_document(Value::Array(commands)), Some(0)).unwrap();
    let error = require(&discovery).unwrap_err();

    assert!(!discovery.order_expiry_in_seconds);
    assert!(error.contains("identity.init, identity.show"), "{error}");
}

#[test]
fn a_discovery_for_another_command_is_refused() {
    let text = discovery_document(Value::Array(vec![])).replace("\"schema\",", "\"order.list\",");

    assert!(Discovery::parse(&text, Some(0)).is_err());
}

#[tokio::test]
async fn discovery_runs_the_executable_offline() {
    let dir = tempfile::tempdir().unwrap();
    let binary = fake_cli(
        dir.path(),
        &discovery_document(Value::Array(every_required())),
    );

    let discovery = discover(&binary, &TokioCliProcess).await.unwrap();

    assert_eq!(discovery.missing(), Vec::<&str>::new());
}

/// `doctor --artifact ./mostro-cli`: the path is the operator's, relative
/// to their directory, not to the temporary one discovery runs in.
#[tokio::test]
async fn a_relative_executable_is_resolved_before_discovery() {
    let dir = tempfile::tempdir().unwrap();
    let binary = fake_cli(
        dir.path(),
        &discovery_document(Value::Array(every_required())),
    );
    let up = std::env::current_dir().unwrap().components().count() - 1;
    let relative =
        std::path::PathBuf::from("../".repeat(up)).join(binary.strip_prefix("/").unwrap());
    assert!(relative.is_relative());

    let discovery = discover(&relative, &TokioCliProcess).await.unwrap();

    assert_eq!(discovery.missing(), Vec::<&str>::new());
}

#[test]
fn a_file_that_cannot_run_is_not_a_cli() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("mostro-cli");
    std::fs::write(&path, b"not executable").unwrap();

    assert!(validate_executable(&path).is_err());
    assert!(validate_executable(&dir.path().join("absent")).is_err());
}
