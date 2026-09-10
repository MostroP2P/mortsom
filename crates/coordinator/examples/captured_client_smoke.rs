//! Starts a copied client bundle without publishing orders or making payments.
use mortsom_coordinator::{desktop, doctor::RealHost};
use mortsom_desktop_driver::{Platform, linux::LinuxSession, server::WebServer, web::WebSession};
use mortsom_ui_driver::{process::TokioRunner, ui::UiSession};
use std::{path::Path, sync::Arc, time::Duration};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args().collect();
    let platform: Platform = args.get(1).ok_or("provide linux or web")?.parse()?;
    let artifact = args
        .get(2)
        .ok_or("provide the existing client artifact")?
        .into();
    let (config, _) = desktop::load(
        Path::new("config/desktop.example.toml"),
        Path::new("stack"),
        &TokioRunner,
        &RealHost,
    )
    .await?;
    let options = desktop::prepare(
        &config,
        platform,
        Some(artifact),
        "Xvfb".into(),
        &TokioRunner,
        &RealHost,
    )
    .await?;
    options.verify().await?;
    let server = if platform == Platform::Web {
        Some(WebServer::start_owned(&options.artifact, Arc::clone(&options.bundle)).await?)
    } else {
        None
    };
    let session: Box<dyn UiSession> = match &server {
        Some(server) => Box::new(WebSession::new(
            Arc::new(TokioRunner),
            server.url().into(),
            options.chromedriver.clone(),
            options.browser.clone(),
        )),
        None => Box::new(LinuxSession::new(
            Arc::new(TokioRunner),
            options.artifact.clone(),
            options.xvfb.clone(),
        )),
    };
    let result = async {
        check_visible_client(session.as_ref()).await?;
        if args.get(3).is_some_and(|arg| arg == "form") {
            check_form(session.as_ref()).await?;
        }
        Ok::<_, Box<dyn std::error::Error>>(())
    }
    .await;
    let client_cleanup = session.end().await;
    let server_cleanup = match server {
        Some(server) => server.stop().await,
        None => Ok(()),
    };
    let captured = options.bundle.root().to_path_buf();
    drop(options);
    result?;
    client_cleanup?;
    server_cleanup?;
    if captured.exists() {
        return Err("captured bundle survived cleanup".into());
    }
    println!(
        "{platform}: copied bundle launched; public marker and order book verified; client/server/bundle cleaned"
    );
    Ok(())
}

async fn check_visible_client(session: &dyn UiSession) -> Result<(), Box<dyn std::error::Error>> {
    println!("starting isolated client from captured bundle");
    session.clear_app().await?;
    session.activate_app().await?;
    session
        .wait_for("env.marker", Duration::from_secs(60))
        .await?;
    session
        .wait_for("onboarding.walkthrough.skip", Duration::from_secs(30))
        .await?;
    session.tap("onboarding.walkthrough.skip").await?;
    session
        .wait_for("order.add.fab", Duration::from_secs(30))
        .await?;
    Ok(())
}

async fn check_form(session: &dyn UiSession) -> Result<(), Box<dyn std::error::Error>> {
    async fn tap(ui: &dyn UiSession, id: &str) -> Result<(), Box<dyn std::error::Error>> {
        ui.wait_for(id, Duration::from_secs(30)).await?;
        ui.tap(id).await?;
        Ok(())
    }
    tap(session, "order.add.fab").await?;
    tap(session, "order.add.sell").await?;
    for (id, value) in [
        ("order.create.fiat_amount", "10"),
        ("order.create.payment_method", "Cash"),
    ] {
        session.wait_for(id, Duration::from_secs(30)).await?;
        session.type_text(id, value).await?;
        if session.text_of(id).await? != value {
            return Err("form readback mismatch".into());
        }
    }
    if !session.exists("order.create.sats_amount").await? {
        tap(session, "order.create.price_type").await?;
    }
    session
        .wait_for("order.create.sats_amount", Duration::from_secs(30))
        .await?;
    session.type_text("order.create.sats_amount", "100").await?;
    if session.text_of("order.create.sats_amount").await? != "100" {
        return Err("sats readback mismatch".into());
    }
    println!("form fiat, method and fixed sats readback passed; no submission");
    Ok(())
}
