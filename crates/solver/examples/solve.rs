//! Operator tool: rules on a dispute against a running stack.
//!
//! `solve <relay> <daemon-pubkey> <settings.toml> take <dispute-id>` or
//! `... cancel|settle <order-id>` (after a take in the same process, since
//! each process is a fresh solver identity: pass both as `take <dispute>
//! then cancel|settle`).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::print_stdout)]
use std::time::Duration;

use mortsom_scenario_kit::context::DisputeSolver;
use mortsom_solver::Solver;
use nostr_sdk::prelude::PublicKey;

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();
    let args: Vec<String> = std::env::args().collect();
    let (relay, daemon, settings, dispute, then) = (
        &args[1],
        PublicKey::parse(&args[2]).expect("daemon pubkey"),
        &args[3],
        &args[4],
        args.get(5).map(String::as_str),
    );
    let settings =
        mortsom_environment::daemon_settings::DaemonSettings::load(std::path::Path::new(settings))
            .expect("settings");
    let nsec = settings.nostr.nsec_privkey.expect("nsec");
    let solver = Solver::connect(relay, daemon, Duration::from_secs(60))
        .await
        .expect("connect");
    solver.register(&nsec).await.expect("register");
    tokio::time::sleep(Duration::from_secs(3)).await;
    let order = solver.take_dispute(dispute).await.expect("take");
    println!("took dispute {dispute}; it is about order {order}");
    match then {
        Some("cancel") => {
            solver.cancel(&order).await.expect("cancel");
            println!("cancelled {order}");
        }
        Some("settle") => {
            solver.settle(&order).await.expect("settle");
            println!("settled {order}");
        }
        _ => {}
    }
    solver.stop().await;
}
