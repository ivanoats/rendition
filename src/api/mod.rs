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
        return (
            StatusCode::NOT_FOUND,
            format!("asset not found: {asset_path}"),
        )
            .into_response();
    }

    let asset = match state.storage.get(&asset_path).await {
        Ok(asset) => asset,
        Err(err) => {
            tracing::error!("storage error fetching {asset_path}: {err:#}");
            return (StatusCode::INTERNAL_SERVER_ERROR, "storage error").into_response();
        }
    };

    match transform::apply(asset.data, params, &asset.content_type).await {
        Ok((bytes, content_type)) => (
            [(header::CONTENT_TYPE, HeaderValue::from_static(content_type))],
            bytes,
        )
            .into_response(),
        Err(err) => {
            tracing::error!("transform error for {asset_path}: {err:#}");
            // Map "format unsupported by this libvips build" errors to 415,
            // distinguishing client-driven format requests from server faults.
            let msg = err.to_string();
            if msg.contains("is not supported by this libvips build") {
                (StatusCode::UNSUPPORTED_MEDIA_TYPE, "format not supported").into_response()
            } else {
                (StatusCode::INTERNAL_SERVER_ERROR, "transform failed").into_response()
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::StatusCode;
    use axum_test::TestServer;
    use std::collections::HashMap;
    use std::sync::Arc;

    use crate::storage::{Asset, StorageBackend};

    // -- MockStorage ---------------------------------------------------------

    /// In-memory storage backend for tests.
    #[derive(Clone)]
    struct MockStorage(HashMap<String, Vec<u8>>);

    impl MockStorage {
        fn empty() -> Self {
            Self(HashMap::new())
        }

        fn with_file(mut self, path: &str, data: Vec<u8>) -> Self {
            self.0.insert(path.to_string(), data);
            self
        }
    }

    impl StorageBackend for MockStorage {
        async fn get(&self, path: &str) -> anyhow::Result<Asset> {
            self.0
                .get(path)
                .map(|data| Asset {
                    size: data.len(),
                    content_type: crate::storage::content_type_from_ext(path).to_string(),
                    data: data.clone(),
                })
                .ok_or_else(|| anyhow::anyhow!("not found: {path}"))
        }

        async fn exists(&self, path: &str) -> bool {
            self.0.contains_key(path)
        }
    }

    fn make_server(storage: MockStorage) -> TestServer {
        let state = AppState {
            storage: Arc::new(storage),
        };
        TestServer::new(router(state)).expect("failed to build test server")
    }

    // -- Tests ---------------------------------------------------------------

    #[tokio::test]
    async fn missing_asset_returns_404() {
        let server = make_server(MockStorage::empty());
        let resp = server.get("/cdn/ghost.jpg").await;
        assert_eq!(resp.status_code(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn existing_jpeg_returns_200_with_correct_content_type() {
        let jpeg = crate::transform::test_jpeg(32, 32);
        let server = make_server(MockStorage::empty().with_file("photo.jpg", jpeg));

        let resp = server.get("/cdn/photo.jpg").await;
        assert_eq!(resp.status_code(), StatusCode::OK);
        assert_eq!(
            resp.headers()
                .get("content-type")
                .and_then(|v| v.to_str().ok()),
            Some("image/jpeg")
        );
    }

    #[tokio::test]
    async fn fmt_webp_returns_webp_content_type() {
        let jpeg = crate::transform::test_jpeg(32, 32);
        let server = make_server(MockStorage::empty().with_file("photo.jpg", jpeg));

        let resp = server
            .get("/cdn/photo.jpg")
            .add_query_param("fmt", "webp")
            .await;
        assert_eq!(resp.status_code(), StatusCode::OK);
        assert_eq!(
            resp.headers()
                .get("content-type")
                .and_then(|v| v.to_str().ok()),
            Some("image/webp")
        );
    }

    #[tokio::test]
    async fn resize_params_return_200() {
        let jpeg = crate::transform::test_jpeg(64, 64);
        let server = make_server(MockStorage::empty().with_file("photo.jpg", jpeg));

        let resp = server
            .get("/cdn/photo.jpg")
            .add_query_param("wid", "16")
            .add_query_param("hei", "16")
            .await;
        assert_eq!(resp.status_code(), StatusCode::OK);
    }

    #[tokio::test]
    async fn invalid_crop_returns_500() {
        let jpeg = crate::transform::test_jpeg(32, 32);
        let server = make_server(MockStorage::empty().with_file("photo.jpg", jpeg));

        let resp = server
            .get("/cdn/photo.jpg")
            .add_query_param("crop", "not,valid")
            .await;
        assert_eq!(resp.status_code(), StatusCode::INTERNAL_SERVER_ERROR);
    }

    #[tokio::test]
    async fn rotate_90_returns_200() {
        let jpeg = crate::transform::test_jpeg(32, 32);
        let server = make_server(MockStorage::empty().with_file("photo.jpg", jpeg));

        let resp = server
            .get("/cdn/photo.jpg")
            .add_query_param("rotate", "90")
            .await;
        assert_eq!(resp.status_code(), StatusCode::OK);
    }

    #[tokio::test]
    async fn flip_hv_returns_200() {
        let jpeg = crate::transform::test_jpeg(32, 32);
        let server = make_server(MockStorage::empty().with_file("photo.jpg", jpeg));

        let resp = server
            .get("/cdn/photo.jpg")
            .add_query_param("flip", "hv")
            .await;
        assert_eq!(resp.status_code(), StatusCode::OK);
    }

    #[tokio::test]
    async fn fmt_avif_returns_avif_content_type() {
        // libvips on some hosts (notably default Ubuntu builds) lacks an AV1
        // encoder linked into libheif. We probe at runtime and skip the
        // assertion path that would otherwise return 415.
        if !crate::transform::avif_supported() {
            eprintln!("skipping fmt_avif test: libvips on this host has no AVIF saver");
            return;
        }

        let jpeg = crate::transform::test_jpeg(32, 32);
        let server = make_server(MockStorage::empty().with_file("photo.jpg", jpeg));

        let resp = server
            .get("/cdn/photo.jpg")
            .add_query_param("fmt", "avif")
            .await;
        assert_eq!(resp.status_code(), StatusCode::OK);
        assert_eq!(
            resp.headers()
                .get("content-type")
                .and_then(|v| v.to_str().ok()),
            Some("image/avif")
        );
    }

    #[tokio::test]
    async fn fmt_avif_unsupported_returns_415() {
        // The complement of the test above: when libvips lacks AVIF support,
        // a request for fmt=avif must return 415 Unsupported Media Type
        // rather than 500 or (worse) aborting the process.
        if crate::transform::avif_supported() {
            eprintln!("skipping unsupported-avif test: libvips on this host has an AVIF saver");
            return;
        }

        let jpeg = crate::transform::test_jpeg(32, 32);
        let server = make_server(MockStorage::empty().with_file("photo.jpg", jpeg));

        let resp = server
            .get("/cdn/photo.jpg")
            .add_query_param("fmt", "avif")
            .await;
        assert_eq!(resp.status_code(), StatusCode::UNSUPPORTED_MEDIA_TYPE);
    }

    #[tokio::test]
    async fn fmt_png_returns_png_content_type() {
        let jpeg = crate::transform::test_jpeg(32, 32);
        let server = make_server(MockStorage::empty().with_file("photo.jpg", jpeg));

        let resp = server
            .get("/cdn/photo.jpg")
            .add_query_param("fmt", "png")
            .await;
        assert_eq!(resp.status_code(), StatusCode::OK);
        assert_eq!(
            resp.headers()
                .get("content-type")
                .and_then(|v| v.to_str().ok()),
            Some("image/png")
        );
    }
}
