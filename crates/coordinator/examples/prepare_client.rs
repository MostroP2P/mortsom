//! Builds and captures a client without launching it or creating an order/payment.
use mortsom_coordinator::{desktop, doctor::RealHost};
use mortsom_desktop_driver::Platform;
use mortsom_ui_driver::process::TokioRunner;
use std::path::Path;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let platform: Platform = std::env::args()
        .nth(1)
        .ok_or("provide linux or web")?
        .parse()?;
    let home = std::env::var("HOME")?;
    let _lock = mortsom_coordinator::lock::RunLock::acquire_host(Path::new(&home))?;
    let stack = std::env::var("MORTSOM_STACK_DIR").unwrap_or_else(|_| "stack".into());
    let source = std::env::args()
        .nth(2)
        .map(|value| mortsom_coordinator::app_source::Selector::parse(&value))
        .transpose()?
        .map(mortsom_coordinator::config::AppSource::Remote);
    let (config, _) = desktop::load_with_source(
        Path::new("config/desktop.example.toml"),
        Path::new(&stack),
        &TokioRunner,
        &RealHost,
        source.as_ref(),
    )
    .await?;
    println!("{platform}: preparing source build against the verified persistent stack");
    let options = desktop::prepare(
        &config,
        platform,
        None,
        "Xvfb".into(),
        &TokioRunner,
        &RealHost,
    )
    .await?;
    options.verify().await?;
    println!(
        "{}",
        serde_json::to_string_pretty(&serde_json::json!({
            "platform": platform,
            "app_build": options.provenance,
            "app_artifact": options.bundle.fingerprint(),
        }))?
    );
    let captured = options.bundle.root().to_path_buf();
    drop(options);
    if captured.exists() {
        return Err("captured bundle survived preparation cleanup".into());
    }
    println!(
        "{platform}: source build and captured bundle verified; temporary bundle removed; no clients/orders/payments"
    );
    Ok(())
}
