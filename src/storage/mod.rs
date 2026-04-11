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

fn content_type_from_ext(path: &str) -> &'static str {
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
    fn get(&self, path: &str) -> impl Future<Output = anyhow::Result<Asset>> + Send {
        let full_path = self.root.join(path);
        let content_type = content_type_from_ext(path).to_string();
        async move {
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
    }

    fn exists(&self, path: &str) -> impl Future<Output = bool> + Send {
        let full_path = self.root.join(path);
        async move { tokio::fs::metadata(&full_path).await.is_ok() }
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
    fn get(&self, _path: &str) -> impl Future<Output = anyhow::Result<Asset>> + Send {
        async { todo!("S3Storage::get not yet implemented") }
    }

    fn exists(&self, _path: &str) -> impl Future<Output = bool> + Send {
        async { todo!("S3Storage::exists not yet implemented") }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // --- content_type_from_ext ---

    #[test]
    fn extension_to_mime_jpeg() {
        assert_eq!(content_type_from_ext("photo.jpg"), "image/jpeg");
        assert_eq!(content_type_from_ext("photo.jpeg"), "image/jpeg");
    }

    #[test]
    fn extension_to_mime_png() {
        assert_eq!(content_type_from_ext("image.png"), "image/png");
    }

    #[test]
    fn extension_to_mime_webp() {
        assert_eq!(content_type_from_ext("image.webp"), "image/webp");
    }

    #[test]
    fn extension_to_mime_svg() {
        assert_eq!(content_type_from_ext("icon.svg"), "image/svg+xml");
    }

    #[test]
    fn extension_to_mime_mp4() {
        assert_eq!(content_type_from_ext("video.mp4"), "video/mp4");
    }

    #[test]
    fn extension_to_mime_unknown() {
        assert_eq!(
            content_type_from_ext("file.xyz"),
            "application/octet-stream"
        );
        assert_eq!(content_type_from_ext("no-extension"), "application/octet-stream");
    }

    // --- LocalStorage ---

    #[tokio::test]
    async fn local_storage_get_existing_file() {
        let dir = tempfile::tempdir().unwrap();
        let filename = "rendition_test_get_existing.png";
        let file_path = dir.path().join(filename);
        let data = b"fake png bytes";
        tokio::fs::write(&file_path, data).await.unwrap();

        let storage = LocalStorage::new(dir.path());
        let asset = storage.get(filename).await.unwrap();

        assert_eq!(asset.data, data);
        assert_eq!(asset.content_type, "image/png");
        assert_eq!(asset.size, data.len());
    }

    #[tokio::test]
    async fn local_storage_get_missing_file() {
        let dir = tempfile::tempdir().unwrap();
        let storage = LocalStorage::new(dir.path());
        let result = storage.get("rendition_test_does_not_exist_xyz.png").await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn local_storage_exists_true() {
        let dir = tempfile::tempdir().unwrap();
        let filename = "rendition_test_exists_true.png";
        let file_path = dir.path().join(filename);
        tokio::fs::write(&file_path, b"data").await.unwrap();

        let storage = LocalStorage::new(dir.path());
        assert!(storage.exists(filename).await);
    }

    #[tokio::test]
    async fn local_storage_exists_false() {
        let dir = tempfile::tempdir().unwrap();
        let storage = LocalStorage::new(dir.path());
        assert!(!storage.exists("rendition_test_absent_xyz.png").await);
    }
}

