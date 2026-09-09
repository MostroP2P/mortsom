//! Public, local connection references of the stack: where each actor's
//! wallet answers and where its credentials sit. Never the credentials.
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::StackError;

/// Host port the relay publishes.
pub const RELAY_PORT: u16 = 7000;
/// Actor wallets and the host port each one's gRPC answers on.
pub const ACTOR_PORTS: [(&str, u16); 2] = [("alice", 10001), ("bob", 10002)];

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Connections {
    /// Host-reachable relay.
    pub relay_url: String,
    /// Actor name to wallet reference.
    pub wallets: BTreeMap<String, WalletReference>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WalletReference {
    /// Host-reachable gRPC endpoint.
    pub grpc_url: String,
    /// TLS certificate path; read by the wallet client only.
    pub cert_file: PathBuf,
    /// Admin macaroon path; read by the wallet client only.
    pub macaroon_file: PathBuf,
}

/// Path of one reserved credential reference under the stack directory.
/// Reads nothing and queries nothing.
pub fn credential_reference(root: &Path, variable: &str) -> Option<PathBuf> {
    for (actor, _) in ACTOR_PORTS {
        for (suffix, file) in [("CERT", "tls.cert"), ("MACAROON", "admin.macaroon")] {
            if variable == format!("MORTSOM_{}_LND_{suffix}", actor.to_uppercase()) {
                return Some(root.join("local/creds").join(actor).join(file));
            }
        }
    }
    None
}

impl Connections {
    /// The references the compose file fixes for a stack rooted at `root`.
    pub fn for_stack(root: &Path) -> Self {
        let wallets = ACTOR_PORTS
            .into_iter()
            .map(|(actor, port)| {
                let creds = root.join("local/creds").join(actor);
                (
                    actor.to_owned(),
                    WalletReference {
                        grpc_url: format!("https://127.0.0.1:{port}"),
                        cert_file: creds.join("tls.cert"),
                        macaroon_file: creds.join("admin.macaroon"),
                    },
                )
            })
            .collect();
        Self {
            relay_url: format!("ws://127.0.0.1:{RELAY_PORT}"),
            wallets,
        }
    }

    /// Actor role to wallet endpoint, the public part of a snapshot.
    pub fn wallet_endpoints(&self) -> BTreeMap<String, String> {
        self.wallets
            .iter()
            .map(|(actor, wallet)| (actor.clone(), wallet.grpc_url.clone()))
            .collect()
    }

    /// Refuses a descriptor whose references left the stack directory or
    /// moved off the loopback ports.
    pub fn validate(&self, root: &Path) -> Result<(), StackError> {
        if *self != Self::for_stack(root) {
            return Err(StackError::Configuration(
                "public stack endpoints or credential references are invalid".into(),
            ));
        }
        Ok(())
    }

    /// The `env:` values configuration resolves.
    pub fn values(&self) -> BTreeMap<String, String> {
        let mut values = BTreeMap::new();
        for (actor, wallet) in &self.wallets {
            let prefix = format!("MORTSOM_{}_LND", actor.to_uppercase());
            values.insert(format!("{prefix}_HOST"), wallet.grpc_url.clone());
            values.insert(
                format!("{prefix}_CERT"),
                wallet.cert_file.display().to_string(),
            );
            values.insert(
                format!("{prefix}_MACAROON"),
                wallet.macaroon_file.display().to_string(),
            );
        }
        values
    }
}
