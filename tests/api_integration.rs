//! API integration tests.
//!
//! Spins up the real Axum router backed by the `assets/` directory and sends
//! HTTP requests through `axum-test`.  No port is bound; all I/O is in-process.

use axum::http::StatusCode;
use axum_test::TestServer;
use rendition::config::AppConfig;
use std::path::PathBuf;

fn make_server() -> TestServer {
    // CARGO_MANIFEST_DIR points to the crate root, so `assets/` is resolvable
    // regardless of the working directory when `cargo test` is run.
    let assets = concat!(env!("CARGO_MANIFEST_DIR"), "/assets");
    let config = AppConfig {
        assets_path: PathBuf::from(assets),
        ..AppConfig::default()
    };
    let app = rendition::build_app(&config);
    TestServer::new(app).unwrap()
}

// ---------------------------------------------------------------------------
// Health check
// ---------------------------------------------------------------------------

#[tokio::test]
async fn health_returns_200() {
    let server = make_server();
    server.get("/health").await.assert_status_ok();
}

// ---------------------------------------------------------------------------
// CDN asset serving
// ---------------------------------------------------------------------------

#[tokio::test]
async fn cdn_sample_png_returns_200_with_png_content_type() {
    let server = make_server();
    let resp = server.get("/cdn/sample.png").await;
    resp.assert_status_ok();
    let ct = resp
        .headers()
        .get("content-type")
        .expect("content-type header missing")
        .to_str()
        .unwrap();
    assert!(ct.contains("image/png"), "expected image/png, got {ct}");
}

#[tokio::test]
async fn cdn_nonexistent_returns_404() {
    let server = make_server();
    server
        .get("/cdn/nonexistent.jpg")
        .await
        .assert_status(StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn cdn_fmt_webp_returns_200_with_webp_content_type() {
    let server = make_server();
    let resp = server
        .get("/cdn/sample.png")
        .add_query_params([("fmt", "webp")])
        .await;
    resp.assert_status_ok();
    let ct = resp
        .headers()
        .get("content-type")
        .expect("content-type header missing")
        .to_str()
        .unwrap();
    assert!(ct.contains("image/webp"), "expected image/webp, got {ct}");
}

#[tokio::test]
async fn cdn_resize_width_returns_200() {
    let server = make_server();
    server
        .get("/cdn/sample.png")
        .add_query_params([("wid", "50")])
        .await
        .assert_status_ok();
}

#[tokio::test]
async fn cdn_resize_crop_returns_200() {
    let server = make_server();
    server
        .get("/cdn/sample.png")
        .add_query_params([("wid", "50"), ("hei", "50"), ("fit", "crop")])
        .await
        .assert_status_ok();
}

#[tokio::test]
async fn cdn_rotate_returns_200() {
    let server = make_server();
    server
        .get("/cdn/sample.png")
        .add_query_params([("rotate", "90")])
        .await
        .assert_status_ok();
}
