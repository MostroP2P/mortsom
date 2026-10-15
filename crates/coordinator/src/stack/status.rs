//! `stack status`: what runs, what each node holds, which identity answers.
use mortsom_ui_driver::process::CommandRunner;

#[cfg(test)]
#[path = "status_tests.rs"]
mod status_tests;

use super::compose::{BASE_SERVICES, Compose, DAEMON_SERVICE, attempt};
use super::descriptor::Descriptor;
use super::layout::Layout;
use super::{StackError, image, nodes};

/// Left margin every line of the report is written from.
const INDENT: &str = "  ";
/// SGR parameters: green for what is up, red for what is not, grey for the
/// rules, and a bold pair for the verdict.
const GREEN: &str = "32";
const RED: &str = "31";
const DIM: &str = "2";
const BOLD_GREEN: &str = "1;32";
const BOLD_RED: &str = "1;31";

/// One Lightning node as `lncli` reports it, or unreachable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Node {
    /// `alice`, `bob` or `carol`.
    pub name: String,
    /// `None` when the node did not answer.
    pub active_channels: Option<usize>,
    /// On-chain balance; `None` when the node did not answer.
    pub confirmed_sats: Option<u64>,
}

/// The stack as observed now; `descriptor` is what `up` recorded.
#[derive(Debug, Clone)]
pub struct Status {
    /// Every service Compose lists for the project.
    pub services: Vec<nodes::Service>,
    /// The three Lightning nodes.
    pub nodes: Vec<Node>,
    /// What `up` recorded, when it completed here.
    pub descriptor: Option<Descriptor>,
    /// The running daemon container is the recorded image.
    pub daemon_matches_image: bool,
}

/// Inspects the project in `layout`. Reads no secret and changes nothing.
pub async fn inspect(runner: &dyn CommandRunner, layout: &Layout) -> Result<Status, StackError> {
    let descriptor = Descriptor::read(layout)?;
    let compose = Compose::new(layout, None);
    // `--all`: without it Compose leaves stopped containers out, and a
    // stopped service would read as removed.
    let table = attempt(
        runner,
        &compose.command(&[
            "--profile",
            "daemon",
            "ps",
            "--all",
            "--format",
            "{{.Service}} {{.State}}",
        ]),
    )
    .await?;
    let services = if table.success {
        nodes::parse_services(&table.stdout)
    } else {
        Vec::new()
    };
    let mut observed = Vec::new();
    for name in ["alice", "bob", "carol"] {
        let channels = attempt(runner, &compose.lncli(name, &["listchannels"])).await?;
        let balance = attempt(runner, &compose.lncli(name, &["walletbalance"])).await?;
        observed.push(Node {
            name: name.to_owned(),
            active_channels: channels
                .success
                .then(|| nodes::active_channel_count(&channels.stdout))
                .transpose()?,
            confirmed_sats: balance
                .success
                .then(|| nodes::confirmed_balance(&balance.stdout))
                .transpose()?,
        });
    }
    let daemon_matches_image = match &descriptor {
        Some(descriptor) => {
            let expected = image::ExpectedImage {
                id: descriptor.daemon_image.image_id.clone(),
                reference: descriptor.daemon_image.reference.clone(),
            };
            image::verify_running(runner, &compose, layout, &expected)
                .await
                .is_ok()
        }
        None => false,
    };
    Ok(Status {
        services,
        nodes: observed,
        descriptor,
        daemon_matches_image,
    })
}

impl Status {
    fn missing_services(&self) -> Vec<&'static str> {
        BASE_SERVICES
            .into_iter()
            .chain(std::iter::once(DAEMON_SERVICE))
            .filter(|name| {
                !self
                    .services
                    .iter()
                    .any(|service| service.name == *name && service.is_running())
            })
            .collect()
    }

    /// Usable by a run: every service running, every node with an active
    /// channel, and the recorded daemon identity actually running.
    pub fn is_ready(&self) -> bool {
        self.missing_services().is_empty()
            && self
                .nodes
                .iter()
                .all(|node| node.active_channels.is_some_and(|count| count > 0))
            && self.descriptor.is_some()
            && self.daemon_matches_image
    }

    /// Plain-text report, one line per service and node, then the verdict.
    /// This is what goes into an error message and what scripts read, so it
    /// never carries escape sequences.
    pub fn render(&self) -> String {
        self.render_with_color(false)
    }

    /// [`render`](Self::render), optionally styled for a terminal.
    ///
    /// A badge is what the eye finds in a screenful of output, so the state
    /// of every service and node leads its line. Colour wraps the badges and
    /// the verdict only: the words themselves — and anything grepping for
    /// them — are the same either way.
    pub fn render_with_color(&self, color: bool) -> String {
        let paint = |code: &str, text: &str| {
            if color {
                format!("\u{1b}[{code}m{text}\u{1b}[0m")
            } else {
                text.to_owned()
            }
        };
        // No fixed width fits this report: a daemon image pinned by digest
        // is longer than any number worth hard-coding, and a stack that is
        // down prints nothing but short lines. The frame is measured off
        // the unpainted body, since escape sequences occupy no columns.
        let bare = |_: &str, text: &str| text.to_owned();
        let rule_width = self
            .body(&bare)
            .lines()
            .map(|line| line.chars().count())
            .max()
            .unwrap_or_default()
            .saturating_sub(INDENT.len());
        let rule = format!("{INDENT}{}\n", paint(DIM, &"─".repeat(rule_width)));
        let mut out = String::new();
        out.push_str(&rule);
        out.push_str(&format!("{INDENT}stack\n"));
        out.push_str(&rule);
        out.push_str(&self.body(&paint));
        out
    }

    /// Everything under the heading: what runs, what each node holds, the
    /// identity in effect and the verdict.
    fn body(&self, paint: &impl Fn(&str, &str) -> String) -> String {
        let mut out = self.services_block(paint);
        out.push_str(&self.nodes_block(paint));
        out.push_str(&self.daemon_block(paint));
        out.push_str(&self.verdict_line(paint));
        out
    }

    /// One line per expected service. A container that was removed rather
    /// than stopped is absent from `ps` entirely, so it reads as gone rather
    /// than being silently left off the report.
    fn services_block(&self, paint: &impl Fn(&str, &str) -> String) -> String {
        let mut out = String::new();
        for name in BASE_SERVICES
            .into_iter()
            .chain(std::iter::once(DAEMON_SERVICE))
        {
            let line = match self.services.iter().find(|service| service.name == name) {
                Some(service) if service.is_running() => {
                    format!("{} {name} ({})", paint(GREEN, "[UP]"), service.state)
                }
                Some(service) => {
                    format!("{} {name} ({})", paint(RED, "[DOWN]"), service.state)
                }
                None => format!("{} {name} (no container)", paint(RED, "[GONE]")),
            };
            out.push_str(&format!("{INDENT}{line}\n"));
        }
        out
    }

    /// One line per node, from two independent `lncli` calls: a node
    /// answering about channels but holding none is up and idle, and one
    /// answering about only half of it has a probe that failed. Both the
    /// detail and the badge are read off the pair, so a green badge never
    /// sits beside a fact that never arrived.
    fn nodes_block(&self, paint: &impl Fn(&str, &str) -> String) -> String {
        let mut out = String::new();
        for node in &self.nodes {
            let (detail, usable) = match (node.active_channels, node.confirmed_sats) {
                (Some(channels), Some(sats)) => (
                    format!("{channels} active channel(s), {sats} confirmed sats"),
                    channels > 0,
                ),
                (Some(channels), None) => (
                    format!("{channels} active channel(s), balance did not answer"),
                    false,
                ),
                (None, Some(sats)) => (
                    format!("channels did not answer, {sats} confirmed sats"),
                    false,
                ),
                (None, None) => ("unreachable".to_owned(), false),
            };
            let badge = if usable {
                paint(GREEN, "[OK]")
            } else {
                paint(RED, "[--]")
            };
            out.push_str(&format!("{INDENT}{badge} lnd-{}: {detail}\n", node.name));
        }
        out
    }

    /// The identity and image in effect: which build is running decides what
    /// a result means, and a container that is not the recorded image makes
    /// every result meaningless.
    fn daemon_block(&self, paint: &impl Fn(&str, &str) -> String) -> String {
        match &self.descriptor {
            Some(descriptor) => {
                let mut out = format!(
                    "{INDENT}daemon identity: {}\n{INDENT}daemon image: {}\n{INDENT}daemon source: {}\n",
                    descriptor.daemon_pubkey,
                    descriptor.daemon_image.reference,
                    descriptor.daemon_source
                );
                if !self.daemon_matches_image {
                    out.push_str(&format!(
                        "{INDENT}{} the running container is not this image\n",
                        paint(RED, "[--]")
                    ));
                }
                out
            }
            None => format!(
                "{INDENT}{} no daemon identity on disk: run `mortsom stack up`\n",
                paint(RED, "[--]")
            ),
        }
    }

    /// The line a tired operator looks at first; it still names what is not
    /// running.
    fn verdict_line(&self, paint: &impl Fn(&str, &str) -> String) -> String {
        let missing = self.missing_services();
        if self.is_ready() {
            return format!("{INDENT}stack: {}\n", paint(BOLD_GREEN, "ready"));
        }
        let verdict = paint(BOLD_RED, "not ready");
        if missing.is_empty() {
            format!("{INDENT}stack: {verdict}\n")
        } else {
            format!(
                "{INDENT}stack: {verdict}; not running: {}\n",
                missing.join(", ")
            )
        }
    }
}
