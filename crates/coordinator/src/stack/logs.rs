//! `stack logs`: the log of one service of the stack, by service name
//! rather than by container name. The container name is Compose's
//! business (project prefix, replica suffix); the operator asks for
//! `mostro` and the plan resolves it through the same project every other
//! stack command uses, after the same ownership check.
use mortsom_android_driver::process::{Command, CommandRunner};

use super::StackError;
use super::compose::{BASE_SERVICES, Compose, DAEMON_SERVICE};
use super::layout::Layout;
use super::ownership;

/// What to show.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogsOptions {
    /// Service name (`mostro`, `relay`, `lnd-alice`, ...).
    pub service: String,
    /// Keep streaming.
    pub follow: bool,
    /// Only entries newer than this (`10m`, `2h`, or a timestamp), as
    /// Docker reads it.
    pub since: Option<String>,
    /// Only the last N lines.
    pub tail: Option<u32>,
}

impl Default for LogsOptions {
    fn default() -> Self {
        Self {
            service: DAEMON_SERVICE.to_owned(),
            follow: false,
            since: None,
            tail: None,
        }
    }
}

/// Every service the stack runs, as `stack logs` names them.
pub fn services() -> Vec<&'static str> {
    let mut all = BASE_SERVICES.to_vec();
    all.push(DAEMON_SERVICE);
    all
}

/// The `docker compose logs` plan for `options`; refuses a service the
/// stack does not run.
pub fn plan(layout: &Layout, options: &LogsOptions) -> Result<Command, StackError> {
    if !services().contains(&options.service.as_str()) {
        return Err(StackError::Request(format!(
            "unknown service `{}`; the stack runs {}",
            options.service,
            services().join(", ")
        )));
    }
    let mut args = vec!["--profile", "daemon", "logs", "--no-log-prefix"];
    if options.follow {
        args.push("--follow");
    }
    let since = options.since.clone();
    if let Some(since) = since.as_deref() {
        args.push("--since");
        args.push(since);
    }
    let tail = options.tail.map(|n| n.to_string());
    if let Some(tail) = tail.as_deref() {
        args.push("--tail");
        args.push(tail);
    }
    args.push(&options.service);
    Ok(Compose::new(layout, None).command(&args))
}

/// The plan for `options` once the project is confirmed to be this
/// directory's. The caller runs it with inherited output: a followed log
/// has no end for a runner to wait on, and these are local regtest
/// service logs, kept by Docker, not run evidence.
pub async fn logs(
    runner: &dyn CommandRunner,
    layout: &Layout,
    options: &LogsOptions,
) -> Result<Command, StackError> {
    ownership::verify(runner, layout).await?;
    plan(layout, options)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn layout() -> (tempfile::TempDir, Layout) {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("compose.yml"), "").unwrap();
        let layout = Layout::new(dir.path()).unwrap();
        (dir, layout)
    }

    #[test]
    fn the_daemon_log_is_the_default_and_options_map_onto_compose_flags() {
        let (_dir, layout) = layout();
        let plain = plan(&layout, &LogsOptions::default()).unwrap();
        assert_eq!(
            plain.args,
            [
                "compose",
                "--profile",
                "daemon",
                "logs",
                "--no-log-prefix",
                "mostro"
            ]
        );
        assert_eq!(
            plain.env["COMPOSE_PROJECT_NAME"],
            super::super::layout::PROJECT
        );

        let followed = plan(
            &layout,
            &LogsOptions {
                service: "relay".into(),
                follow: true,
                since: Some("10m".into()),
                tail: Some(50),
            },
        )
        .unwrap();
        assert_eq!(
            followed.args,
            [
                "compose",
                "--profile",
                "daemon",
                "logs",
                "--no-log-prefix",
                "--follow",
                "--since",
                "10m",
                "--tail",
                "50",
                "relay"
            ]
        );
    }

    #[test]
    fn a_service_the_stack_does_not_run_is_refused_by_name() {
        let (_dir, layout) = layout();
        let error = plan(
            &layout,
            &LogsOptions {
                service: "mostrod".into(),
                ..LogsOptions::default()
            },
        )
        .unwrap_err();
        let text = error.to_string();
        assert!(text.contains("unknown service `mostrod`"), "{text}");
        assert!(
            text.contains("mostro") && text.contains("lnd-carol"),
            "{text}"
        );
    }
}
