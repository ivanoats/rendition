//! Rendition library crate.
//!
//! Exposes [`build_app`] so the binary and integration tests can share the
//! same router construction logic.

pub mod api;
pub mod cache;
pub mod config;
pub mod metrics;
pub mod storage;
pub mod transform;

use api::AppState;
use axum::{routing::get, Json, Router};
use cache::MokaTransformCache;
use config::AppConfig;
use metrics::Metrics;
use serde_json::{json, Value};
use std::sync::Arc;
use storage::LocalStorage;
use tower_http::trace::TraceLayer;

/// Build the Axum application router from a loaded [`AppConfig`].
///
/// Constructs and wires all Unit 3 components:
/// - [`LocalStorage`] (or S3 in Unit 2) as the storage backend
/// - [`MokaTransformCache`] seeded from `config.cache_max_entries` and `config.cache_ttl()`
/// - [`Metrics`] counters for cache hit/miss tracking
///
/// Subsequent units (embargo, middleware, observability) will extend this
/// function to wire their components.
pub fn build_app(config: &AppConfig) -> Router {
    let state = AppState {
        storage: Arc::new(LocalStorage::new(&config.assets_path)),
        cache: Arc::new(MokaTransformCache::new(
            config.cache_max_entries,
            config.cache_ttl(),
        )),
        metrics: Arc::new(Metrics::new()),
    };
    Router::new()
        .route("/health", get(health_check))
        .merge(api::router(state))
        .layer(TraceLayer::new_for_http())
}

async fn health_check() -> Json<Value> {
    Json(json!({ "status": "ok", "service": "rendition" }))
}
