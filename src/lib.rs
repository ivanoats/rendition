//! Rendition library — exposes core modules and the app builder for testing.

pub mod api;
pub mod storage;
pub mod transform;

use axum::{routing::get, Json, Router};
use serde_json::{json, Value};
use std::sync::Arc;
use tower_http::trace::TraceLayer;

use api::AppState;
use storage::LocalStorage;

async fn health_check() -> Json<Value> {
    Json(json!({ "status": "ok", "service": "rendition" }))
}

/// Build the full Axum application rooted at `assets_path`.
///
/// Exposed as a library function so integration tests can spin up the real
/// router without binding a port.
pub fn build_app(assets_path: &str) -> Router {
    let state = AppState {
        storage: Arc::new(LocalStorage::new(assets_path)),
    };
    Router::new()
        .route("/health", get(health_check))
        .merge(api::router(state))
        .layer(TraceLayer::new_for_http())
}
