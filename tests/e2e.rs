//! End-to-end tests for the Rendition HTTP API.
//!
//! These tests spin up the full application router (real `LocalStorage` + real
//! libvips transform pipeline) against a temporary asset directory and drive it
//! via [`axum_test::TestServer`].

use axum::http::StatusCode;
use axum_test::TestServer;
use libvips::{ops, VipsApp};
use std::fs;
use std::sync::OnceLock;
use tempfile::TempDir;

// Keep a process-lifetime VipsApp so fixture creation doesn't trigger
// vips_shutdown.  The transform pipeline creates its own static VipsApp via
// the same pattern; libvips handles double-init as a no-op.
static TEST_VIPS: OnceLock<VipsApp> = OnceLock::new();

fn ensure_vips() {
    TEST_VIPS.get_or_init(|| VipsApp::new("rendition_e2e", false).expect("vips init failed"));
}

fn make_fixture_jpeg(w: i32, h: i32) -> Vec<u8> {
    ensure_vips();
    let img = ops::black_with_opts(w, h, &ops::BlackOptions { bands: 3 })
        .expect("failed to create fixture image");
    ops::jpegsave_buffer(&img).expect("failed to encode fixture JPEG")
}

fn setup() -> (TempDir, TestServer) {
    let dir = TempDir::new().expect("failed to create temp dir");
    let jpeg = make_fixture_jpeg(64, 64);
    fs::write(dir.path().join("sample.jpg"), &jpeg).expect("failed to write fixture");

    let root = dir.path().to_string_lossy();
    let app = rendition::build_app(root.as_ref());
    let server = TestServer::new(app).expect("failed to build test server");
    (dir, server)
}

// ---------------------------------------------------------------------------
// Health endpoint
// ---------------------------------------------------------------------------

#[tokio::test]
async fn health_returns_ok() {
    let (_dir, server) = setup();
    let resp = server.get("/health").await;
    assert_eq!(resp.status_code(), StatusCode::OK);
    let body: serde_json::Value = resp.json();
    assert_eq!(body["status"], "ok");
    assert_eq!(body["service"], "rendition");
}

// ---------------------------------------------------------------------------
// Asset serving
// ---------------------------------------------------------------------------

#[tokio::test]
async fn cdn_serves_existing_jpeg() {
    let (_dir, server) = setup();
    let resp = server.get("/cdn/sample.jpg").await;
    assert_eq!(resp.status_code(), StatusCode::OK);
    assert_eq!(
        resp.headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok()),
        Some("image/jpeg")
    );
    assert!(!resp.as_bytes().is_empty());
}

#[tokio::test]
async fn cdn_returns_404_for_missing_asset() {
    let (_dir, server) = setup();
    let resp = server.get("/cdn/does-not-exist.jpg").await;
    assert_eq!(resp.status_code(), StatusCode::NOT_FOUND);
}

// ---------------------------------------------------------------------------
// Format conversion
// ---------------------------------------------------------------------------

#[tokio::test]
async fn cdn_converts_to_webp() {
    let (_dir, server) = setup();
    let resp = server
        .get("/cdn/sample.jpg")
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
async fn cdn_converts_to_avif() {
    if !rendition::transform::avif_supported() {
        eprintln!("skipping cdn_converts_to_avif: libvips on this host has no AVIF saver");
        return;
    }
    let (_dir, server) = setup();
    let resp = server
        .get("/cdn/sample.jpg")
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
async fn cdn_converts_to_png() {
    let (_dir, server) = setup();
    let resp = server
        .get("/cdn/sample.jpg")
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

// ---------------------------------------------------------------------------
// Resize
// ---------------------------------------------------------------------------

#[tokio::test]
async fn cdn_resizes_by_width() {
    let (_dir, server) = setup();
    let resp = server
        .get("/cdn/sample.jpg")
        .add_query_param("wid", "32")
        .await;
    assert_eq!(resp.status_code(), StatusCode::OK);
    assert!(!resp.as_bytes().is_empty());
}

#[tokio::test]
async fn cdn_resizes_with_crop_fit() {
    let (_dir, server) = setup();
    let resp = server
        .get("/cdn/sample.jpg")
        .add_query_param("wid", "20")
        .add_query_param("hei", "40")
        .add_query_param("fit", "crop")
        .await;
    assert_eq!(resp.status_code(), StatusCode::OK);
}

// ---------------------------------------------------------------------------
// Quality
// ---------------------------------------------------------------------------

#[tokio::test]
async fn cdn_accepts_quality_param() {
    let (_dir, server) = setup();
    let resp = server
        .get("/cdn/sample.jpg")
        .add_query_param("fmt", "webp")
        .add_query_param("qlt", "50")
        .await;
    assert_eq!(resp.status_code(), StatusCode::OK);
}

// ---------------------------------------------------------------------------
// Rotate & flip
// ---------------------------------------------------------------------------

#[tokio::test]
async fn cdn_rotates_90_degrees() {
    let (_dir, server) = setup();
    let resp = server
        .get("/cdn/sample.jpg")
        .add_query_param("rotate", "90")
        .await;
    assert_eq!(resp.status_code(), StatusCode::OK);
}

#[tokio::test]
async fn cdn_flips_horizontally() {
    let (_dir, server) = setup();
    let resp = server
        .get("/cdn/sample.jpg")
        .add_query_param("flip", "h")
        .await;
    assert_eq!(resp.status_code(), StatusCode::OK);
}

// ---------------------------------------------------------------------------
// Error cases
// ---------------------------------------------------------------------------

#[tokio::test]
async fn cdn_returns_500_for_invalid_crop() {
    let (_dir, server) = setup();
    let resp = server
        .get("/cdn/sample.jpg")
        .add_query_param("crop", "bad,data")
        .await;
    assert_eq!(resp.status_code(), StatusCode::INTERNAL_SERVER_ERROR);
}
