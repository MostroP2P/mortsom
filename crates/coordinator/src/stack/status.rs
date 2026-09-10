//! `stack status`: what runs, what each node holds, which identity answers.
use mortsom_ui_driver::process::CommandRunner;

use super::compose::{BASE_SERVICES, Compose, DAEMON_SERVICE, attempt};
use super::descriptor::Descriptor;
use super::layout::Layout;
use super::{StackError, image, nodes};

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
    let table = attempt(
        runner,
        &compose.command(&[
            "--profile",
            "daemon",
            "ps",
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

    /// Human report; one line per service and node, then the verdict.
    pub fn render(&self) -> String {
        let mut out = String::from("services\n");
        for name in BASE_SERVICES
            .into_iter()
            .chain(std::iter::once(DAEMON_SERVICE))
        {
            let state = self
                .services
                .iter()
                .find(|service| service.name == name)
                .map_or("absent", |service| service.state.as_str());
            out.push_str(&format!("  {name:<10} {state}\n"));
        }
        out.push_str("nodes\n");
        for node in &self.nodes {
            let line = match (node.active_channels, node.confirmed_sats) {
                (Some(channels), Some(sats)) => {
                    format!("{channels} active channel(s), {sats} confirmed sats")
                }
                _ => "unreachable".to_owned(),
            };
            out.push_str(&format!("  {:<10} {line}\n", node.name));
        }
        match &self.descriptor {
            Some(descriptor) => {
                out.push_str(&format!("daemon      {}\n", descriptor.daemon_pubkey));
                out.push_str(&format!(
                    "image       {}\n",
                    descriptor.daemon_image.reference
                ));
                if !self.daemon_matches_image {
                    out.push_str("            the running container is not this image\n");
                }
            }
            None => {
                out.push_str("daemon      no identity; `mortsom stack up` has not completed here\n")
            }
        }
        let missing = self.missing_services();
        if self.is_ready() {
            out.push_str("stack       ready\n");
        } else if missing.is_empty() {
            out.push_str("stack       not ready\n");
        } else {
            out.push_str(&format!(
                "stack       not ready; not running: {}\n",
                missing.join(", ")
            ));
        }
        out
    }
}
