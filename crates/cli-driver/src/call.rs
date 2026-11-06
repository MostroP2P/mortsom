//! One CLI invocation as Mortsom builds it: the command words, its
//! arguments, whether it talks to the node, whether it is confirmed, and the
//! invoice it carries on standard input.

use mortsom_common::Secret;

/// The node a command talks to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NodeTarget {
    /// Daemon public key (hex or npub).
    pub mostro_pubkey: String,
    /// Relays, comma-joined for `-r`.
    pub relays: Vec<String>,
    /// How long the CLI waits for the node's reply (`--reply-timeout`),
    /// when the built CLI offers the option.
    pub reply_timeout_secs: Option<u64>,
}

/// One invocation.
#[derive(Debug, Clone)]
pub struct Call {
    words: Vec<String>,
    args: Vec<String>,
    node: bool,
    confirmed: bool,
    invoice: Option<Secret>,
}

impl Call {
    /// A command that only touches the actor's local store.
    pub fn local(words: &[&str]) -> Self {
        Self::new(words, false)
    }

    /// A command that talks to the node.
    pub fn node(words: &[&str]) -> Self {
        Self::new(words, true)
    }

    fn new(words: &[&str], node: bool) -> Self {
        Self {
            words: words.iter().map(|w| (*w).to_owned()).collect(),
            args: Vec::new(),
            node,
            confirmed: false,
            invoice: None,
        }
    }

    /// Adds `--flag value`.
    pub fn arg(mut self, flag: &str, value: impl Into<String>) -> Self {
        self.args.push(format!("--{flag}"));
        self.args.push(value.into());
        self
    }

    /// Adds `--flag value` when `value` is present.
    pub fn arg_opt(self, flag: &str, value: Option<impl Into<String>>) -> Self {
        match value {
            Some(value) => self.arg(flag, value),
            None => self,
        }
    }

    /// Answers the command's confirmation in advance (`--yes`). The machine
    /// interface never prompts; a write without it is refused.
    pub fn confirmed(mut self) -> Self {
        self.confirmed = true;
        self
    }

    /// Passes `invoice` as `--invoice -`, writing it to standard input so it
    /// never appears in the process arguments.
    pub fn invoice(mut self, invoice: &Secret) -> Self {
        self.args.push("--invoice".into());
        self.args.push("-".into());
        self.invoice = Some(invoice.clone());
        self
    }

    /// The dotted command name the envelope must carry (`order.take-sell`).
    pub fn name(&self) -> String {
        self.words.join(".")
    }

    /// The secret for standard input, if any.
    pub fn stdin(&self) -> Option<&Secret> {
        self.invoice.as_ref()
    }

    /// The full argument vector.
    pub fn argv(&self, node: &NodeTarget) -> Vec<String> {
        let mut argv = Vec::new();
        if self.node {
            argv.extend([
                "--mostro-pubkey".to_owned(),
                node.mostro_pubkey.clone(),
                "--relays".to_owned(),
                node.relays.join(","),
                "--transport".to_owned(),
                "nip44".to_owned(),
            ]);
            if let Some(secs) = node.reply_timeout_secs {
                argv.extend(["--reply-timeout".to_owned(), secs.to_string()]);
            }
        }
        argv.extend(["--output".to_owned(), "json".to_owned()]);
        argv.extend(self.words.iter().cloned());
        argv.extend(self.args.iter().cloned());
        if self.confirmed {
            argv.push("--yes".into());
        }
        argv
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn target() -> NodeTarget {
        NodeTarget {
            mostro_pubkey: "ab".repeat(32),
            relays: vec!["ws://127.0.0.1:7500".into()],
            reply_timeout_secs: None,
        }
    }

    #[test]
    fn a_reply_budget_goes_to_node_commands_only() {
        let patient = NodeTarget {
            reply_timeout_secs: Some(60),
            ..target()
        };

        let node = Call::node(&["order", "release"]).argv(&patient);
        let local = Call::local(&["identity", "show"]).argv(&patient);

        assert!(node.windows(2).any(|w| w == ["--reply-timeout", "60"]));
        assert!(!local.contains(&"--reply-timeout".to_owned()));
    }

    #[test]
    fn a_node_command_names_its_node_relays_and_transport() {
        let call = Call::node(&["order", "release"])
            .arg("order", "o1")
            .confirmed();

        let argv = call.argv(&target());

        assert_eq!(call.name(), "order.release");
        assert_eq!(
            argv,
            [
                "--mostro-pubkey",
                &"ab".repeat(32),
                "--relays",
                "ws://127.0.0.1:7500",
                "--transport",
                "nip44",
                "--output",
                "json",
                "order",
                "release",
                "--order",
                "o1",
                "--yes"
            ]
        );
    }

    #[test]
    fn a_local_command_names_no_node() {
        let argv = Call::local(&["identity", "show"]).argv(&target());

        assert_eq!(argv, ["--output", "json", "identity", "show"]);
    }

    #[test]
    fn an_invoice_travels_on_standard_input_never_in_the_arguments() {
        let invoice = Secret::new("lnbcrt1secret");
        let call = Call::node(&["order", "add-invoice"])
            .arg("order", "o1")
            .invoice(&invoice)
            .confirmed();

        let argv = call.argv(&target());

        assert!(argv.windows(2).any(|w| w == ["--invoice", "-"]));
        assert!(!argv.iter().any(|a| a.contains("secret")));
        assert_eq!(call.stdin().map(Secret::expose), Some("lnbcrt1secret"));
    }

    #[test]
    fn an_optional_argument_is_added_only_when_present() {
        let with = Call::node(&["order", "take-sell"]).arg_opt("amount", Some("20"));
        let without = Call::node(&["order", "take-sell"]).arg_opt("amount", None::<String>);

        assert!(with.argv(&target()).contains(&"--amount".to_owned()));
        assert!(!without.argv(&target()).contains(&"--amount".to_owned()));
    }
}
