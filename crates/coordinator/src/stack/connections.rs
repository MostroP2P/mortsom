//! Validation of public, local connection references returned by Ortsom.
use super::StackError;
use serde::Deserialize;
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

#[derive(Deserialize)]
pub(super) struct Connections {
    pub relay_url: String,
    wallets: BTreeMap<String, WalletReference>,
}

#[derive(Deserialize)]
struct WalletReference {
    grpc_url: String,
    cert_file: PathBuf,
    macaroon_file: PathBuf,
}

/// Offline path for one reserved credential reference in a canonical Ortsom checkout.
/// This reads no credential and does not query the stack.
pub fn credential_reference(root: &Path, variable: &str) -> Option<PathBuf> {
    for actor in ["alice", "bob"] {
        for (suffix, file) in [("CERT", "tls.cert"), ("MACAROON", "admin.macaroon")] {
            if variable == format!("MORTSOM_{}_LND_{suffix}", actor.to_uppercase()) {
                return Some(root.join("ci/regtest/creds").join(actor).join(file));
            }
        }
    }
    None
}

impl Connections {
    pub(super) fn resolve(
        self,
        root: &Path,
    ) -> Result<(u16, BTreeMap<String, String>), StackError> {
        let relay_port = local_port(&self.relay_url, "ws")?;
        let mut values = BTreeMap::new();
        let mut ports = Vec::new();
        for actor in ["alice", "bob"] {
            let reference = self.wallets.get(actor).ok_or_else(invalid)?;
            let port = local_port(&reference.grpc_url, "https")?;
            if port == relay_port || ports.contains(&port) {
                return Err(invalid());
            }
            ports.push(port);
            let prefix = format!("MORTSOM_{}_LND", actor.to_uppercase());
            values.insert(format!("{prefix}_HOST"), reference.grpc_url.clone());
            for (suffix, actual) in [
                ("CERT", &reference.cert_file),
                ("MACAROON", &reference.macaroon_file),
            ] {
                let variable = format!("{prefix}_{suffix}");
                if *actual != credential_reference(root, &variable).ok_or_else(invalid)? {
                    return Err(invalid());
                }
                values.insert(variable, actual.display().to_string());
            }
        }
        Ok((relay_port, values))
    }
}

fn local_port(value: &str, scheme: &str) -> Result<u16, StackError> {
    value
        .strip_prefix(&format!("{scheme}://127.0.0.1:"))
        .and_then(|s| s.parse::<u16>().ok())
        .filter(|p| *p > 0)
        .ok_or_else(invalid)
}

fn invalid() -> StackError {
    StackError::Configuration("public stack endpoints or credential references are invalid".into())
}
