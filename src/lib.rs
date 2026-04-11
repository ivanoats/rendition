//! Rendition library crate.
//!
//! Exposes [`build_app`] so the binary and integration tests can share
//! the same router construction logic.

pub mod api;
pub mod cache;
pub mod config;
pub mod metrics;
pub mod storage;
pub mod transform;

use api::AppState;
use axum::{routing::get, Json, Router};
use cache::MokaTransformCache;
use config::{AppConfig, StorageBackendKind};
use metrics::Metrics;
use serde_json::{json, Value};
use std::sync::Arc;
use storage::{LocalStorage, S3Storage, StorageBackend};
use tower_http::trace::TraceLayer;

/// Error returned from [`build_app`] when a backend fails to initialise.
#[derive(Debug, thiserror::Error)]
pub enum AppBuildError {
    /// An S3 backend could not be constructed (missing config,
    /// credential chain failure, etc.).
    #[error("failed to build S3 backend: {0}")]
    S3(#[from] storage::StorageError),
}

/// Build the Axum application router from a loaded [`AppConfig`].
///
/// Unit 2: `storage_backend` selects between the local filesystem and
/// S3 adapters. Unit 3 wires the in-process transform cache and metrics.
pub async fn build_app(config: &AppConfig) -> Result<Router, AppBuildError> {
    let cache = Arc::new(MokaTransformCache::new(
        config.cache_max_entries,
        config.cache_ttl(),
    ));
    let metrics = Arc::new(Metrics::new());

    let router = match config.storage_backend {
        StorageBackendKind::Local => {
            let storage = LocalStorage::new(&config.assets_path, config.local_timeout_ms);
            wire_router(storage, cache, metrics)
        }
        StorageBackendKind::S3 => {
            let storage = S3Storage::new(&config.s3).await?;
            wire_router(storage, cache, metrics)
        }
    };
    Ok(router)
}

fn wire_router<S>(
    storage: S,
    cache: Arc<dyn cache::TransformCache>,
    metrics: Arc<Metrics>,
) -> Router
where
    S: StorageBackend + Clone + Send + Sync + 'static,
{
    let state = AppState {
        storage: Arc::new(storage),
        cache,
        metrics,
    };
    Router::new()
        .route("/health", get(health_check))
        .merge(api::router(state))
        .layer(TraceLayer::new_for_http())
}

async fn health_check() -> Json<Value> {
    Json(json!({ "status": "ok", "service": "rendition" }))
}
