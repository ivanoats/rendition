//! Rendition — open source enterprise media CDN
//!
//! Entry point: starts the Axum HTTP server and registers all routes.

use axum::{routing::get, Extension, Json, Router};
use serde_json::{json, Value};
use std::net::SocketAddr;
use std::sync::Arc;
use tower_http::trace::TraceLayer;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

use storage::LocalStorage;

mod api;
mod storage;
mod transform;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Initialise structured logging.  Set RUST_LOG to control verbosity.
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "rendition=debug,tower_http=debug".into()),
        )
        .with(tracing_subscriber::fmt::layer())
        .init();

    // Resolve asset root — override with RENDITION_ASSETS_PATH env var.
    let assets_path =
        std::env::var("RENDITION_ASSETS_PATH").unwrap_or_else(|_| "./assets".into());
    tracing::info!("Asset root: {assets_path}");

    let storage: Arc<LocalStorage> = Arc::new(LocalStorage::new(&assets_path));

    let app = Router::new()
        .route("/health", get(health_check))
        .merge(api::router())
        .layer(Extension(storage))
        .layer(TraceLayer::new_for_http());

    let addr = SocketAddr::from(([0, 0, 0, 0], 3000));
    tracing::info!("Rendition listening on {addr}");

    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;

    Ok(())
}

/// GET /health — liveness probe
async fn health_check() -> Json<Value> {
    Json(json!({ "status": "ok", "service": "rendition" }))
}
