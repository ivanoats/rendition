//! Storage adapters.
//!
//! Rendition is storage-agnostic. This module defines the
//! [`StorageBackend`] trait and ships concrete adapters:
//!
//! * [`LocalStorage`] — local filesystem (development / on-prem). Lives
//!   in the `local` sub-module.
//! * [`S3Storage`] — AWS S3 / S3-compatible object store. Lives in the
//!   `s3` sub-module.
//!
//! The [`CircuitBreaker`] fault-tolerance primitive used by `S3Storage`
//! lives in its own sub-module (`circuit_breaker`) and is reusable by
//! any future remote backend.
//!
//! ## Error taxonomy (ADR-0004 revised, Unit 2)
//!
//! Every storage operation returns [`Result<T, StorageError>`]. The
//! typed variants let HTTP callers distinguish "asset not found" (404)
//! from "backend unreachable" (503) from "circuit open" (503, fail-fast)
//! without downcasting.

use std::future::Future;
use std::time::Duration;

pub mod circuit_breaker;
pub mod local;
pub mod s3;

pub use local::LocalStorage;
pub use s3::S3Storage;

// ---------------------------------------------------------------------------
// Asset DTO
// ---------------------------------------------------------------------------

/// A raw media asset fetched from a backend.
///
/// `size` is the length of `data` in bytes. For range fetches,
/// `size == data.len() == (range.end - range.start)`, not the full object
/// size on the backend.
#[derive(Debug)]
pub struct Asset {
    /// Raw bytes of the media file.
    pub data: Vec<u8>,
    /// MIME type, e.g. `image/jpeg`.
    pub content_type: String,
    /// Size of [`data`](Asset::data) in bytes.
    pub size: usize,
}

// ---------------------------------------------------------------------------
// StorageError
// ---------------------------------------------------------------------------

/// Typed errors returned by every [`StorageBackend`] method.
///
/// Variants map to HTTP statuses in the request handler (Unit 4):
/// `NotFound` → 404, `InvalidPath` → 400,
/// `CircuitOpen` | `Unavailable` → 503, `Timeout` → 504, `Other` → 500.
#[derive(Debug, thiserror::Error)]
pub enum StorageError {
    /// Asset is absent from the backend. Maps to HTTP 404.
    #[error("asset not found")]
    NotFound,

    /// The logical path is malformed (empty, contains NUL bytes, etc.).
    /// Maps to HTTP 400.
    #[error("invalid path: {reason}")]
    InvalidPath { reason: String },

    /// Circuit breaker is open — the call was rejected without touching
    /// the backend. Maps to HTTP 503 fail-fast.
    #[error("circuit breaker open")]
    CircuitOpen,

    /// I/O deadline exceeded. Maps to HTTP 504.
    #[error("timeout ({op})")]
    Timeout {
        /// The operation that timed out: `get` / `exists` / `get_range`.
        op: &'static str,
    },

    /// Transient backend failure (5xx, throttling, connection error).
    /// Maps to HTTP 503.
    #[error("backend unavailable: {source}")]
    Unavailable {
        /// The underlying cause. Not exposed to HTTP callers — logged
        /// server-side only via `tracing::error!`.
        #[source]
        source: Box<dyn std::error::Error + Send + Sync + 'static>,
    },

    /// Any other failure. Maps to HTTP 500.
    #[error("storage error: {source}")]
    Other {
        /// The underlying cause. Not exposed to HTTP callers.
        #[source]
        source: Box<dyn std::error::Error + Send + Sync + 'static>,
    },
}

impl PartialEq for StorageError {
    /// Cheap equality on the discriminant-only variants. Variants that
    /// carry inner errors (`Unavailable`, `Other`) are never equal — use
    /// `matches!` in tests for those.
    fn eq(&self, other: &Self) -> bool {
        use StorageError::*;
        match (self, other) {
            (NotFound, NotFound) => true,
            (CircuitOpen, CircuitOpen) => true,
            (InvalidPath { reason: a }, InvalidPath { reason: b }) => a == b,
            (Timeout { op: a }, Timeout { op: b }) => a == b,
            _ => false,
        }
    }
}

// ---------------------------------------------------------------------------
// StorageMetrics trait (stub — Unit 7 replaces with Prometheus)
// ---------------------------------------------------------------------------

/// Outcome discriminant recorded on every storage call. One variant per
/// terminal branch of Flows 1–3 in the functional design.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Success,
    NotFound,
    Unavailable,
    Timeout,
    CircuitOpen,
    InvalidPath,
    Other,
}

/// Metrics port. Unit 2 wires [`NoopMetrics`]; Unit 7 drops in a real
/// Prometheus implementation without touching `s3.rs` or
/// `circuit_breaker.rs`.
pub trait StorageMetrics: Send + Sync + 'static {
    /// Record the outcome and duration of a storage operation.
    fn record(&self, op: &str, outcome: Outcome, duration: Duration);
    /// Update the circuit-breaker open/closed gauge.
    fn set_circuit_open(&self, open: bool);
}

/// No-op [`StorageMetrics`] implementation used during Unit 2 before
/// Unit 7 introduces real Prometheus wiring.
pub struct NoopMetrics;

impl StorageMetrics for NoopMetrics {
    fn record(&self, _op: &str, _outcome: Outcome, _duration: Duration) {}
    fn set_circuit_open(&self, _open: bool) {}
}

// ---------------------------------------------------------------------------
// StorageBackend trait
// ---------------------------------------------------------------------------

/// Trait implemented by every storage backend.
///
/// Methods return `impl Future + Send` (RPITIT) so that callers in
/// generic contexts (e.g. axum handlers) can rely on the futures being
/// `Send` without requiring Return Type Notation to express that bound.
/// Concrete `impl` blocks use `async fn` directly — Rust 1.75+ allows
/// this.
pub trait StorageBackend: Send + Sync {
    /// Retrieve the full asset for `path`.
    fn get(&self, path: &str) -> impl Future<Output = Result<Asset, StorageError>> + Send;

    /// Return whether the asset exists, without downloading its body.
    fn exists(&self, path: &str) -> impl Future<Output = Result<bool, StorageError>> + Send;

    /// Retrieve a byte range of the asset. Default implementation fetches
    /// the full asset and slices it; `S3Storage` overrides to pass the
    /// native `Range` header (ADR-0018).
    fn get_range(
        &self,
        path: &str,
        range: std::ops::Range<u64>,
    ) -> impl Future<Output = Result<Asset, StorageError>> + Send {
        async move {
            if range.start >= range.end {
                return Err(StorageError::InvalidPath {
                    reason: format!(
                        "empty or inverted range: start={} end={}",
                        range.start, range.end
                    ),
                });
            }
            let full = self.get(path).await?;
            let start = range.start as usize;
            let end = (range.end as usize).min(full.data.len());
            let slice = full.data.get(start..end).unwrap_or(&[]).to_vec();
            let size = slice.len();
            Ok(Asset {
                data: slice,
                content_type: full.content_type,
                size,
            })
        }
    }
}

// ---------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------

/// Compose an S3-style object key from a prefix and a logical path.
///
/// Implements R-07 / E5 of the functional design:
/// 1. Reject empty paths and paths containing NUL bytes.
/// 2. Strip any leading `/` from the logical path.
/// 3. Normalise the prefix — append `/` if non-empty and missing it.
/// 4. Return `prefix + path`.
///
/// This is the only place the `logical path → S3 key` mapping is
/// defined. `LocalStorage` uses its own path-safety helper
/// (`local::safe_join`) because filesystem semantics differ.
pub(crate) fn compose_key(prefix: &str, path: &str) -> Result<String, StorageError> {
    if path.is_empty() {
        return Err(StorageError::InvalidPath {
            reason: "empty".into(),
        });
    }
    if path.contains('\0') {
        return Err(StorageError::InvalidPath {
            reason: "null byte".into(),
        });
    }
    let trimmed = path.trim_start_matches('/');
    if trimmed.is_empty() {
        return Err(StorageError::InvalidPath {
            reason: "empty after stripping leading /".into(),
        });
    }
    if prefix.is_empty() {
        Ok(trimmed.to_string())
    } else if prefix.ends_with('/') {
        Ok(format!("{prefix}{trimmed}"))
    } else {
        Ok(format!("{prefix}/{trimmed}"))
    }
}

/// Infer a MIME type from the extension of a logical path.
pub(crate) fn content_type_from_ext(path: &str) -> &'static str {
    let lower = path.to_lowercase();
    match lower.rsplit('.').next().unwrap_or("") {
        "jpg" | "jpeg" => "image/jpeg",
        "png" => "image/png",
        "webp" => "image/webp",
        "gif" => "image/gif",
        "svg" => "image/svg+xml",
        "avif" => "image/avif",
        "mp4" => "video/mp4",
        "webm" => "video/webm",
        "mov" => "video/quicktime",
        "pdf" => "application/pdf",
        _ => "application/octet-stream",
    }
}

// ---------------------------------------------------------------------------
// Tests (shared helpers only — backend-specific tests live in their modules)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // -- content_type_from_ext -----------------------------------------------

    #[test]
    fn content_type_jpeg() {
        assert_eq!(content_type_from_ext("photo.jpg"), "image/jpeg");
        assert_eq!(content_type_from_ext("photo.JPEG"), "image/jpeg");
        assert_eq!(content_type_from_ext("photo.jpeg"), "image/jpeg");
    }

    #[test]
    fn content_type_png() {
        assert_eq!(content_type_from_ext("image.png"), "image/png");
    }

    #[test]
    fn content_type_webp() {
        assert_eq!(content_type_from_ext("image.webp"), "image/webp");
    }

    #[test]
    fn content_type_avif() {
        assert_eq!(content_type_from_ext("image.avif"), "image/avif");
    }

    #[test]
    fn content_type_gif() {
        assert_eq!(content_type_from_ext("anim.gif"), "image/gif");
    }

    #[test]
    fn content_type_svg() {
        assert_eq!(content_type_from_ext("icon.svg"), "image/svg+xml");
    }

    #[test]
    fn content_type_video() {
        assert_eq!(content_type_from_ext("clip.mp4"), "video/mp4");
        assert_eq!(content_type_from_ext("clip.webm"), "video/webm");
        assert_eq!(content_type_from_ext("clip.mov"), "video/quicktime");
    }

    #[test]
    fn content_type_pdf() {
        assert_eq!(content_type_from_ext("doc.pdf"), "application/pdf");
    }

    #[test]
    fn content_type_unknown_falls_back_to_octet_stream() {
        assert_eq!(
            content_type_from_ext("file.xyz"),
            "application/octet-stream"
        );
        assert_eq!(
            content_type_from_ext("noextension"),
            "application/octet-stream"
        );
    }

    // -- compose_key ---------------------------------------------------------

    #[test]
    fn compose_key_empty_prefix() {
        assert_eq!(
            compose_key("", "products/shoe.jpg").unwrap(),
            "products/shoe.jpg"
        );
    }

    #[test]
    fn compose_key_prefix_without_trailing_slash() {
        assert_eq!(
            compose_key("assets", "products/shoe.jpg").unwrap(),
            "assets/products/shoe.jpg"
        );
    }

    #[test]
    fn compose_key_prefix_with_trailing_slash() {
        assert_eq!(
            compose_key("assets/", "products/shoe.jpg").unwrap(),
            "assets/products/shoe.jpg"
        );
    }

    #[test]
    fn compose_key_strips_leading_slash_from_path() {
        assert_eq!(
            compose_key("assets/", "/products/shoe.jpg").unwrap(),
            "assets/products/shoe.jpg"
        );
    }

    #[test]
    fn compose_key_rejects_empty_path() {
        let err = compose_key("assets/", "").unwrap_err();
        assert!(matches!(err, StorageError::InvalidPath { .. }));
    }

    #[test]
    fn compose_key_rejects_null_byte() {
        let err = compose_key("", "foo\0bar.jpg").unwrap_err();
        assert!(matches!(err, StorageError::InvalidPath { .. }));
    }

    #[test]
    fn compose_key_rejects_only_slashes() {
        let err = compose_key("", "/").unwrap_err();
        assert!(matches!(err, StorageError::InvalidPath { .. }));
    }

    // -- StorageError equality -----------------------------------------------

    #[test]
    fn storage_error_partial_eq_simple_variants() {
        assert_eq!(StorageError::NotFound, StorageError::NotFound);
        assert_eq!(StorageError::CircuitOpen, StorageError::CircuitOpen);
        assert_eq!(
            StorageError::Timeout { op: "get" },
            StorageError::Timeout { op: "get" }
        );
        assert_ne!(
            StorageError::Timeout { op: "get" },
            StorageError::Timeout { op: "exists" }
        );
    }
}
