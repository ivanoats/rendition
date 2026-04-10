//! Rendition library crate.
//!
//! Exposes [`build_app`] so the binary and integration tests can share the
//! same router construction logic.

pub mod api;
pub mod config;
pub mod storage;
pub mod transform;

use api::AppState;
use axum::{routing::get, Json, Router};
use config::AppConfig;
use serde_json::{json, Value};
use std::sync::Arc;
use storage::LocalStorage;
use tower_http::trace::TraceLayer;

/// Build the Axum application router from a loaded [`AppConfig`].
///
/// For Unit 1 only `assets_path` is read from the config; subsequent units
/// (S3 backend, transform cache, embargo, middleware, observability) will
/// extend this function to wire their components into the router.
pub fn build_app(config: &AppConfig) -> Router {
    let state = AppState {
        storage: Arc::new(LocalStorage::new(&config.assets_path)),
    };
    Router::new()
        .route("/health", get(health_check))
        .merge(api::router(state))
        .layer(TraceLayer::new_for_http())
}

async fn health_check() -> Json<Value> {
    Json(json!({ "status": "ok", "service": "rendition" }))
}
