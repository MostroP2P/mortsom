//! Builds and captures a client without launching it or creating an order/payment.
use mortsom_android_driver::process::TokioRunner;
use mortsom_coordinator::{desktop, doctor::RealHost};
use mortsom_desktop_driver::Platform;
use std::path::Path;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let platform: Platform = std::env::args()
        .nth(1)
        .ok_or("provide linux or web")?
        .parse()?;
    let (config, _) = desktop::load(
        Path::new("config/desktop.example.toml"),
        Path::new("stack"),
        &TokioRunner,
        &RealHost,
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
