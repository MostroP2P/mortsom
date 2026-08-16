//! `mortsom` command-line entry point.

use std::process::ExitCode;

use clap::Parser;
use mortsom_coordinator::cli::{Cli, run};

#[tokio::main]
async fn main() -> ExitCode {
    // Picked before anything opens a `wss://` connection: relays and
    // wallets are reached over TLS, and rustls will not choose a provider
    // on its own when several are compiled in.
    mortsom_common::tls::install_crypto_provider();
    let cli = Cli::parse();
    run(cli).await
}
