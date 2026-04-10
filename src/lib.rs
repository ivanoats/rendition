//! Rendition library crate.
//!
//! Exposes [`build_app`] so the binary and integration tests can share the
//! same router construction logic.

pub mod api;
pub mod storage;
pub mod transform;

use api::AppState;
use axum::{routing::get, Json, Router};
use serde_json::{json, Value};
use std::sync::Arc;
use storage::LocalStorage;
use tower_http::trace::TraceLayer;

/// Build the Axum application router wired to [`LocalStorage`] at `assets_path`.
pub fn build_app(assets_path: &str) -> Router {
    let state = AppState {
        storage: Arc::new(LocalStorage::new(assets_path)),
    };
    Router::new()
        .route("/health", get(health_check))
        .merge(api::router(state))
        .layer(TraceLayer::new_for_http())
}

async fn health_check() -> Json<Value> {
    Json(json!({ "status": "ok", "service": "rendition" }))
}

