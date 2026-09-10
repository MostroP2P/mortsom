//! Read-only browser accessibility smoke; no order and no payment.
use mortsom_desktop_driver::{server::WebServer, web::WebSession};
use mortsom_ui_driver::{process::TokioRunner, ui::UiSession};
use std::{path::PathBuf, sync::Arc, time::Duration};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();
    let root = PathBuf::from(args.get(1).ok_or("provide Web bundle directory")?);
    let driver = PathBuf::from(args.get(2).ok_or("provide ChromeDriver executable")?);
    let browser = PathBuf::from(args.get(3).ok_or("provide Chromium executable")?);
    let server = WebServer::start(&root).await?;
    let session = WebSession::new(Arc::new(TokioRunner), server.url().into(), driver, browser);
    let result = async {
        session.clear_app().await?;
        session
            .wait_for("env.marker", Duration::from_secs(60))
            .await?;
        println!("Web test environment marker is visible");
        Ok::<_, Box<dyn std::error::Error>>(())
    }
    .await;
    let cleanup = session.end().await;
    result?;
    cleanup?;
    Ok(())
}
