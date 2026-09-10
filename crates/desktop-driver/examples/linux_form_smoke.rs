//! Native order-form editing diagnostic. Cancels the form without publishing it.
use mortsom_desktop_driver::linux::LinuxSession;
use mortsom_ui_driver::{process::TokioRunner, ui::UiSession};
use std::{path::PathBuf, sync::Arc, time::Duration};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();
    let executable = PathBuf::from(args.get(1).ok_or("provide app executable")?);
    let xvfb = PathBuf::from(args.get(2).map(String::as_str).unwrap_or("Xvfb"));
    let session = LinuxSession::new(Arc::new(TokioRunner), executable, xvfb);
    let result = edit_form(&session).await;
    if result.is_err() {
        match session.screenshot_png().await {
            Ok(bytes) => {
                if let Err(error) = std::fs::write("/tmp/mortsom-linux-form.png", bytes) {
                    eprintln!("cannot write form diagnostic: {error}");
                }
            }
            Err(error) => eprintln!("cannot capture form diagnostic: {error}"),
        }
        match session.page_source().await {
            Ok(tree) => eprintln!("{}", mortsom_common::Redactor::default().redact(&tree)),
            Err(error) => eprintln!("cannot collect diagnostic tree: {error}"),
        }
        match session.text_of("order.create.sats_amount").await {
            Ok(text) => eprintln!(
                "sats field readback: length={}, matches expected={}",
                text.len(),
                text == "100"
            ),
            Err(error) => eprintln!("cannot read sats field: {error}"),
        }
    }
    let cleanup = session.end().await;
    result?;
    cleanup?;
    println!("Native form values verified; form canceled and session cleaned up");
    Ok(())
}

async fn tap(session: &dyn UiSession, id: &str) -> Result<(), Box<dyn std::error::Error>> {
    println!("tap: {id}");
    session.wait_for(id, Duration::from_secs(30)).await?;
    session.tap(id).await?;
    Ok(())
}

async fn edit_form(session: &dyn UiSession) -> Result<(), Box<dyn std::error::Error>> {
    println!("prepare isolated native session");
    session.clear_app().await?;
    session.activate_app().await?;
    session
        .wait_for("env.marker", Duration::from_secs(60))
        .await?;
    tap(session, "onboarding.walkthrough.skip").await?;
    tap(session, "order.add.fab").await?;
    tap(session, "order.add.sell").await?;
    for (id, value) in [
        ("order.create.fiat_amount", "10"),
        ("order.create.payment_method", "Cash"),
    ] {
        session.wait_for(id, Duration::from_secs(30)).await?;
        println!("edit: {id}");
        session.type_text(id, value).await?;
        if session.text_of(id).await? != value {
            return Err("form value did not match".into());
        }
    }
    if !session.exists("order.create.sats_amount").await? {
        tap(session, "order.create.price_type").await?;
    }
    session.type_text("order.create.sats_amount", "100").await?;
    if session.text_of("order.create.sats_amount").await? != "100" {
        return Err("sats value did not match".into());
    }
    tap(session, "order.create.cancel").await?;
    session
        .wait_for("order.add.fab", Duration::from_secs(30))
        .await?;
    Ok(())
}
