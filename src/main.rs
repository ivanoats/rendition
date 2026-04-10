//! Rendition — open source enterprise media CDN
//!
//! Entry point: initialises logging, resolves configuration, and starts the
//! Axum HTTP server.  Application logic lives in the `rendition` library crate.

use std::net::SocketAddr;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "rendition=debug,tower_http=debug".into()),
        )
        .with(tracing_subscriber::fmt::layer())
        .init();

    // Resolve asset root — override with RENDITION_ASSETS_PATH env var.
    let assets_path = std::env::var("RENDITION_ASSETS_PATH").unwrap_or_else(|_| "./assets".into());
    tracing::info!("Asset root: {assets_path}");

    let app = rendition::build_app(&assets_path);

    let addr = SocketAddr::from(([0, 0, 0, 0], 3000));
    tracing::info!("Rendition listening on {addr}");

    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;

    Ok(())
}
