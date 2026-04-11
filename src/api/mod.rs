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
//!
//! ## Caching
//!
//! `serve_asset` performs a cache lookup **before** hitting storage.  On a
//! hit the response is returned directly from [`AppState::cache`] without
//! fetching the asset or running the libvips pipeline.  On a miss the result
//! is stored in the cache after a successful transform.
//!
//! Cache hits and misses are tracked via [`AppState::metrics`].

use axum::{
    extract::{Path, Query, State},
    http::{header, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    routing::get,
    Router,
};
use std::sync::Arc;

use crate::{
    cache::{self, CachedResponse, TransformCache},
    metrics::Metrics,
    storage::{StorageBackend, StorageError},
    transform::{self, TransformParams},
};

/// Shared application state injected into every handler via axum's [`State`] extractor.
#[derive(Clone)]
pub struct AppState<S> {
    pub storage: Arc<S>,
    /// In-process transform cache — keyed on SHA-256 of (path, params).
    pub cache: Arc<dyn TransformCache>,
    /// Operational counters (cache hits/misses, etc.).
    pub metrics: Arc<Metrics>,
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
/// Pipeline (per component design C-07):
/// 1. Compute cache key from `(path, params)`.
/// 2. **Cache hit** → return cached bytes + record hit metric.
/// 3. **Cache miss** → record miss metric, continue.
/// 4. Fetch raw bytes from storage backend (→ 404 if missing).
/// 5. Run the transform pipeline (→ 500 on failure).
/// 6. Store the result in the cache.
/// 7. Stream the result with the correct `Content-Type`.
async fn serve_asset<S>(
    State(state): State<AppState<S>>,
    Path(asset_path): Path<String>,
    Query(params): Query<TransformParams>,
) -> Response
where
    S: StorageBackend,
{
    // ── Step 1: cache lookup ────────────────────────────────────────────────
    let cache_key = cache::compute_cache_key(&asset_path, &params);

    if let Some(cached) = state.cache.get(&cache_key) {
        state.metrics.record_cache_hit();
        return (
            [(
                header::CONTENT_TYPE,
                HeaderValue::from_static(cached.content_type),
            )],
            cached.data,
        )
            .into_response();
    }
    state.metrics.record_cache_miss();

    // ── Step 2: storage fetch ───────────────────────────────────────────────
    match state.storage.exists(&asset_path).await {
        Ok(true) => {}
        Ok(false) => {
            return (
                StatusCode::NOT_FOUND,
                format!("asset not found: {asset_path}"),
            )
                .into_response();
        }
        Err(err) => return storage_error_response(&asset_path, err),
    }

    let asset = match state.storage.get(&asset_path).await {
        Ok(asset) => asset,
        Err(err) => return storage_error_response(&asset_path, err),
    };

    // ── Step 3: transform ───────────────────────────────────────────────────
    match transform::apply(asset.data, params, &asset.content_type).await {
        Ok((bytes, content_type)) => {
            // Store the successful response in the cache.
            state.cache.put(
                cache_key,
                &asset_path,
                CachedResponse {
                    data: bytes.clone(),
                    content_type,
                },
            );
            (
                [(
                    header::CONTENT_TYPE,
                    HeaderValue::from_static(content_type),
                )],
                bytes,
            )
                .into_response()
        }
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

/// Map a [`StorageError`] to an HTTP response.
///
/// Full detail is logged server-side via `tracing::error!`; the HTTP
/// response carries only a generic status text so AWS request IDs,
/// bucket names, and internal error chains stay inside the server
/// (SECURITY-09 hardening). Unit 4 will refine the mapping when it
/// owns the request handler; this implementation is the minimum needed
/// to keep the existing project compiling after the `StorageBackend`
/// trait return-type change.
fn storage_error_response(asset_path: &str, err: StorageError) -> Response {
    tracing::error!("storage error fetching {asset_path}: {err:#}");
    match err {
        StorageError::NotFound => (
            StatusCode::NOT_FOUND,
            format!("asset not found: {asset_path}"),
        )
            .into_response(),
        StorageError::InvalidPath { .. } => {
            (StatusCode::BAD_REQUEST, "invalid asset path").into_response()
        }
        StorageError::CircuitOpen | StorageError::Unavailable { .. } => {
            (StatusCode::SERVICE_UNAVAILABLE, "storage unavailable").into_response()
        }
        StorageError::Timeout { .. } => {
            (StatusCode::GATEWAY_TIMEOUT, "storage timeout").into_response()
        }
        StorageError::Other { .. } => {
            (StatusCode::INTERNAL_SERVER_ERROR, "storage error").into_response()
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
    use std::time::Duration;

    use crate::cache::MokaTransformCache;
    use crate::storage::{Asset, StorageBackend, StorageError};

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
        async fn get(&self, path: &str) -> Result<Asset, StorageError> {
            self.0
                .get(path)
                .map(|data| Asset {
                    size: data.len(),
                    content_type: crate::storage::content_type_from_ext(path).to_string(),
                    data: data.clone(),
                })
                .ok_or(StorageError::NotFound)
        }

        async fn exists(&self, path: &str) -> Result<bool, StorageError> {
            Ok(self.0.contains_key(path))
        }
    }

    // -- Test helpers --------------------------------------------------------

    fn make_state(storage: MockStorage) -> AppState<MockStorage> {
        AppState {
            storage: Arc::new(storage),
            cache: Arc::new(MokaTransformCache::new(100, Duration::from_secs(3600))),
            metrics: Arc::new(Metrics::new()),
        }
    }

    fn make_server(storage: MockStorage) -> TestServer {
        TestServer::new(router(make_state(storage))).expect("failed to build test server")
    }

    // -- Existing behaviour (regression) ------------------------------------

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

    // -- Cache behaviour (Unit 3 acceptance criteria) -----------------------

    /// FR-03: A second identical request must be served from cache.
    ///
    /// We verify this by asserting that after the second request the cache-hit
    /// counter is exactly 1 (and the miss counter is exactly 1, proving only
    /// the first request hit storage).
    #[tokio::test]
    async fn second_request_hits_cache() {
        let jpeg = crate::transform::test_jpeg(32, 32);
        let state = make_state(MockStorage::empty().with_file("photo.jpg", jpeg));
        let metrics = Arc::clone(&state.metrics);
        let server = TestServer::new(router(state)).expect("failed to build test server");

        // First request → cache miss → libvips transform → cache store.
        let resp1 = server.get("/cdn/photo.jpg").await;
        assert_eq!(resp1.status_code(), StatusCode::OK);
        assert_eq!(metrics.cache_misses_total(), 1, "first request should be a miss");
        assert_eq!(metrics.cache_hits_total(), 0, "no hits yet");

        // Second identical request → cache hit → no libvips invocation.
        let resp2 = server.get("/cdn/photo.jpg").await;
        assert_eq!(resp2.status_code(), StatusCode::OK);
        assert_eq!(metrics.cache_hits_total(), 1, "second request should be a cache hit");
        assert_eq!(metrics.cache_misses_total(), 1, "miss count must not increase");

        // Both responses must carry the same bytes.
        assert_eq!(
            resp1.as_bytes(),
            resp2.as_bytes(),
            "cached response must be byte-for-byte identical to the original"
        );
    }

    /// FR-03: `rendition_cache_hits_total` increments on a cache hit.
    #[tokio::test]
    async fn cache_hit_increments_metric() {
        let jpeg = crate::transform::test_jpeg(32, 32);
        let state = make_state(MockStorage::empty().with_file("photo.jpg", jpeg));
        let metrics = Arc::clone(&state.metrics);
        let server = TestServer::new(router(state)).expect("failed to build test server");

        server.get("/cdn/photo.jpg").await; // prime the cache
        server.get("/cdn/photo.jpg").await; // cache hit
        server.get("/cdn/photo.jpg").await; // cache hit

        assert_eq!(metrics.cache_hits_total(), 2);
    }

    /// FR-03: `rendition_cache_misses_total` increments on a cache miss.
    #[tokio::test]
    async fn cache_miss_increments_metric() {
        let jpeg = crate::transform::test_jpeg(32, 32);
        let state = make_state(MockStorage::empty().with_file("photo.jpg", jpeg));
        let metrics = Arc::clone(&state.metrics);
        let server = TestServer::new(router(state)).expect("failed to build test server");

        server.get("/cdn/photo.jpg").await; // miss (first request)

        assert_eq!(metrics.cache_misses_total(), 1);
        assert_eq!(metrics.cache_hits_total(), 0);
    }

    /// FR-03: Different transform params produce separate cache entries.
    #[tokio::test]
    async fn different_params_are_cached_separately() {
        let jpeg = crate::transform::test_jpeg(64, 64);
        let state = make_state(MockStorage::empty().with_file("photo.jpg", jpeg));
        let metrics = Arc::clone(&state.metrics);
        let server = TestServer::new(router(state)).expect("failed to build test server");

        server
            .get("/cdn/photo.jpg")
            .add_query_param("wid", "32")
            .await;
        server
            .get("/cdn/photo.jpg")
            .add_query_param("wid", "16")
            .await;

        // Both are distinct cache keys → two misses, zero hits.
        assert_eq!(metrics.cache_misses_total(), 2);
        assert_eq!(metrics.cache_hits_total(), 0);

        // Now repeat each → two hits.
        server
            .get("/cdn/photo.jpg")
            .add_query_param("wid", "32")
            .await;
        server
            .get("/cdn/photo.jpg")
            .add_query_param("wid", "16")
            .await;

        assert_eq!(metrics.cache_hits_total(), 2);
    }

    /// FR-03: A 404 response must NOT be cached.
    #[tokio::test]
    async fn missing_asset_not_cached() {
        let state = make_state(MockStorage::empty());
        let metrics = Arc::clone(&state.metrics);
        let server = TestServer::new(router(state)).expect("failed to build test server");

        server.get("/cdn/ghost.jpg").await; // 404
        server.get("/cdn/ghost.jpg").await; // should also be a miss, not a hit

        assert_eq!(metrics.cache_hits_total(), 0, "404 responses must not be cached");
        assert_eq!(metrics.cache_misses_total(), 2, "each 404 request should be a miss");
    }
}
