//! Storage adapters.
//!
//! Rendition is storage-agnostic.  This module defines the [`StorageBackend`]
//! trait and ships concrete adapters for:
//!
//! * [`LocalStorage`] — local filesystem (development / on-prem)
//! * [`S3Storage`] — Amazon S3 / S3-compatible (stub, not yet implemented)

use std::future::Future;
use std::path::PathBuf;

/// A raw media asset fetched from a backend.
pub struct Asset {
    /// Raw bytes of the media file.
    pub data: Vec<u8>,
    /// MIME type, e.g. `image/jpeg`.
    pub content_type: String,
    /// Size of [`data`](Asset::data) in bytes.
    pub size: usize,
}

/// Trait implemented by every storage backend.
///
/// Methods return `impl Future + Send` (RPITIT) so that callers in generic
/// contexts (e.g. axum handlers) can rely on the futures being `Send` without
/// requiring nightly Return Type Notation (RTN) to express that bound.
/// Concrete `impl` blocks use `async fn` directly — Rust 1.75+ allows this.
pub trait StorageBackend: Send + Sync {
    /// Retrieve an asset by its logical path (e.g. `"products/shoe.jpg"`).
    ///
    /// Returns an error when the asset does not exist or cannot be read.
    fn get(&self, path: &str) -> impl Future<Output = anyhow::Result<Asset>> + Send;

    /// Return `true` if the asset exists in this backend.
    fn exists(&self, path: &str) -> impl Future<Output = bool> + Send;
}

// ---------------------------------------------------------------------------
// Content-type detection
// ---------------------------------------------------------------------------

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
// LocalStorage
// ---------------------------------------------------------------------------

/// Reads assets from a directory on the local filesystem.
///
/// Paths are resolved relative to [`root`](LocalStorage::root).
/// Configurable via the `RENDITION_ASSETS_PATH` environment variable.
#[derive(Clone)]
pub struct LocalStorage {
    root: PathBuf,
}

impl LocalStorage {
    /// Create a new adapter rooted at `root`.
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }
}

impl StorageBackend for LocalStorage {
    async fn get(&self, path: &str) -> anyhow::Result<Asset> {
        let full_path = self.root.join(path);
        let content_type = content_type_from_ext(path).to_string();
        let data = tokio::fs::read(&full_path)
            .await
            .map_err(|e| anyhow::anyhow!("cannot read {}: {}", full_path.display(), e))?;
        let size = data.len();
        Ok(Asset {
            data,
            content_type,
            size,
        })
    }

    async fn exists(&self, path: &str) -> bool {
        tokio::fs::metadata(self.root.join(path)).await.is_ok()
    }
}

// ---------------------------------------------------------------------------
// S3Storage (stub)
// ---------------------------------------------------------------------------

/// Stub for S3-compatible object storage (AWS S3, MinIO, Cloudflare R2).
///
/// Not yet implemented — present to demonstrate the pluggable backend pattern.
#[allow(dead_code)]
pub struct S3Storage {
    pub bucket: String,
    pub region: String,
}

impl S3Storage {
    pub fn new(bucket: impl Into<String>, region: impl Into<String>) -> Self {
        Self {
            bucket: bucket.into(),
            region: region.into(),
        }
    }
}

impl StorageBackend for S3Storage {
    async fn get(&self, _path: &str) -> anyhow::Result<Asset> {
        todo!("S3Storage::get not yet implemented")
    }

    async fn exists(&self, _path: &str) -> bool {
        todo!("S3Storage::exists not yet implemented")
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

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

    // -- LocalStorage --------------------------------------------------------

    #[tokio::test]
    async fn exists_returns_true_for_present_file() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("test.jpg"), b"data").unwrap();
        let storage = LocalStorage::new(dir.path());
        assert!(storage.exists("test.jpg").await);
    }

    #[tokio::test]
    async fn exists_returns_false_for_missing_file() {
        let dir = TempDir::new().unwrap();
        let storage = LocalStorage::new(dir.path());
        assert!(!storage.exists("ghost.jpg").await);
    }

    #[tokio::test]
    async fn get_returns_correct_bytes_and_content_type() {
        let dir = TempDir::new().unwrap();
        let payload = b"fake jpeg payload";
        fs::write(dir.path().join("photo.jpg"), payload).unwrap();
        let storage = LocalStorage::new(dir.path());

        let asset = storage.get("photo.jpg").await.unwrap();
        assert_eq!(asset.data, payload);
        assert_eq!(asset.content_type, "image/jpeg");
        assert_eq!(asset.size, payload.len());
    }

    #[tokio::test]
    async fn get_returns_correct_content_type_for_png() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("banner.png"), b"png data").unwrap();
        let storage = LocalStorage::new(dir.path());

        let asset = storage.get("banner.png").await.unwrap();
        assert_eq!(asset.content_type, "image/png");
    }

    #[tokio::test]
    async fn get_returns_error_for_missing_file() {
        let dir = TempDir::new().unwrap();
        let storage = LocalStorage::new(dir.path());
        assert!(storage.get("ghost.jpg").await.is_err());
    }

    #[tokio::test]
    async fn get_size_matches_file_length() {
        let dir = TempDir::new().unwrap();
        let data = vec![0u8; 1024];
        fs::write(dir.path().join("big.jpg"), &data).unwrap();
        let storage = LocalStorage::new(dir.path());

        let asset = storage.get("big.jpg").await.unwrap();
        assert_eq!(asset.size, 1024);
        assert_eq!(asset.data.len(), 1024);
    }
}
