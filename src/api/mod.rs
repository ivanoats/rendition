//! URL-based transform API.
//!
//! Routes follow Scene7 URL conventions so existing integrations can migrate
//! with minimal changes.
//!
//! ## URL format
//!
//! ```text
//! GET /cdn/{asset_path}?wid=800&hei=600&fit=crop&fmt=webp&qlt=85
//! ```
//!
//! | Parameter | Description                                               | Default     |
//! |-----------|-----------------------------------------------------------|-------------|
//! | `wid`     | Output width (px)                                         | original    |
//! | `hei`     | Output height (px)                                        | original    |
//! | `fit`     | Fit mode: `crop` · `fit` · `stretch` · `constrain`       | `constrain` |
//! | `fmt`     | Output format: `webp` · `avif` · `jpeg` · `png`          | original    |
//! | `qlt`     | Quality 1–100 (lossy formats)                             | `85`        |
//! | `crop`    | Pre-resize crop region `x,y,w,h`                          | none        |

use axum::{
    extract::{Path, Query, State},
    http::{header, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    routing::get,
    Router,
};
use std::sync::Arc;

use crate::{
    storage::StorageBackend,
    transform::{self, TransformParams},
};

/// Shared application state injected into every handler via axum's [`State`] extractor.
#[derive(Clone)]
pub struct AppState<S> {
    pub storage: Arc<S>,
}

/// Returns the sub-router for all `/cdn/…` transform endpoints.
pub fn router<S>(state: AppState<S>) -> Router
where
    S: StorageBackend + Clone + Send + Sync + 'static,
{
    Router::new()
        .route("/cdn/*asset_path", get(serve_asset::<S>))
        .with_state(state)
}

/// Serve and optionally transform a media asset.
///
/// 1. Parses transform parameters from the query string (→ 400 on bad input).
/// 2. Fetches raw bytes from the storage backend (→ 404 if missing).
/// 3. Runs the transform pipeline (→ 500 on failure).
/// 4. Streams the result with the correct `Content-Type`.
async fn serve_asset<S>(
    State(state): State<AppState<S>>,
    Path(asset_path): Path<String>,
    Query(params): Query<TransformParams>,
) -> Response
where
    S: StorageBackend,
{
    if !state.storage.exists(&asset_path).await {
        return (StatusCode::NOT_FOUND, format!("asset not found: {asset_path}")).into_response();
    }

    let asset = match state.storage.get(&asset_path).await {
        Ok(asset) => asset,
        Err(err) => {
            tracing::error!("storage error fetching {asset_path}: {err:#}");
            return (StatusCode::INTERNAL_SERVER_ERROR, "storage error").into_response();
        }
    };

    match transform::apply(asset.data, params).await {
        Ok((bytes, content_type)) => (
            [(
                header::CONTENT_TYPE,
                HeaderValue::from_static(content_type),
            )],
            bytes,
        )
            .into_response(),
        Err(err) => {
            tracing::error!("transform error for {asset_path}: {err:#}");
            (StatusCode::INTERNAL_SERVER_ERROR, "transform failed").into_response()
        }
    }
}
