//! Read-only native accessibility smoke. Creates no order and makes no payment.
use mortsom_desktop_driver::linux::LinuxSession;
use mortsom_ui_driver::{process::TokioRunner, ui::UiSession};
use std::{path::PathBuf, sync::Arc, time::Duration};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();
    let executable = PathBuf::from(args.get(1).ok_or("provide app executable")?);
    let xvfb = PathBuf::from(args.get(2).map(String::as_str).unwrap_or("Xvfb"));
    let session = LinuxSession::new(Arc::new(TokioRunner), executable, xvfb);
    let result = async {
        session.clear_app().await?;
        session.activate_app().await?;
        session
            .wait_for("env.marker", Duration::from_secs(60))
            .await?;
        let source = session.page_source().await?;
        println!("{}", mortsom_common::Redactor::default().redact(&source));
        Ok::<_, Box<dyn std::error::Error>>(())
    }
    .await;
    if result.is_err()
        && let Ok(tree) = session.page_source().await
    {
        eprintln!("{}", mortsom_common::Redactor::default().redact(&tree));
    }
    let cleanup = session.end().await;
    result?;
    cleanup?;
    Ok(())
}
