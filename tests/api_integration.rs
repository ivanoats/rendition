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

/// Decode PNG bytes and return `(width, height)`.
fn png_dims(bytes: &[u8]) -> (u32, u32) {
    let img = image::load_from_memory(bytes)
        .unwrap_or_else(|e| panic!("failed to decode image: {e}"));
    (img.width(), img.height())
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
async fn cdn_resize_width_returns_correct_dimensions() {
    let server = make_server();

    // Capture the original dimensions.
    let orig_resp = server.get("/cdn/sample.png").await;
    orig_resp.assert_status_ok();
    let (orig_w, _orig_h) = png_dims(orig_resp.as_bytes());

    // Request a width smaller than the original.
    let target_w: u32 = (orig_w / 2).max(1);
    let resp = server
        .get("/cdn/sample.png")
        .add_query_params([("wid", &target_w.to_string())])
        .await;
    resp.assert_status_ok();
    let (out_w, _out_h) = png_dims(resp.as_bytes());
    assert_eq!(out_w, target_w, "output width should equal the requested width");
}

#[tokio::test]
async fn cdn_resize_crop_returns_exact_dimensions() {
    let server = make_server();
    let resp = server
        .get("/cdn/sample.png")
        .add_query_params([("wid", "30"), ("hei", "20"), ("fit", "crop")])
        .await;
    resp.assert_status_ok();
    let (out_w, out_h) = png_dims(resp.as_bytes());
    assert_eq!(out_w, 30, "cropped width should be exactly 30");
    assert_eq!(out_h, 20, "cropped height should be exactly 20");
}

#[tokio::test]
async fn cdn_rotate_90_swaps_dimensions() {
    let server = make_server();

    let orig_resp = server.get("/cdn/sample.png").await;
    orig_resp.assert_status_ok();
    let (orig_w, orig_h) = png_dims(orig_resp.as_bytes());

    let rotated_resp = server
        .get("/cdn/sample.png")
        .add_query_params([("rotate", "90")])
        .await;
    rotated_resp.assert_status_ok();
    let (rot_w, rot_h) = png_dims(rotated_resp.as_bytes());

    assert_eq!(rot_w, orig_h, "rotated width should equal original height");
    assert_eq!(rot_h, orig_w, "rotated height should equal original width");
}
