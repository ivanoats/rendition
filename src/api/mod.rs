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

use axum::{extract::Path, routing::get, Router};

/// Returns the sub-router for all `/cdn/…` transform endpoints.
pub fn router() -> Router {
    Router::new().route("/cdn/*asset_path", get(serve_asset))
}

/// Serve and optionally transform a media asset.
///
/// The asset path is resolved against the configured storage backend.
/// Transform parameters are parsed from the query string.
async fn serve_asset(Path(asset_path): Path<String>) -> String {
    // TODO: resolve asset from storage backend, apply transforms, stream response
    format!("TODO: serve and transform /{asset_path}")
}
