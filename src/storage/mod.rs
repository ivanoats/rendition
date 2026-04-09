//! Storage adapters.
//!
//! Rendition is storage-agnostic.  This module defines the [`StorageBackend`]
//! trait and will ship concrete adapters for:
//!
//! * Local filesystem (for development / on-prem)
//! * Amazon S3 / S3-compatible (MinIO, Cloudflare R2)
//! * Google Cloud Storage
//! * Azure Blob Storage

use std::future::Future;

/// A raw media asset fetched from a backend.
pub struct Asset {
    /// Raw bytes of the media file.
    pub data: Vec<u8>,
    /// MIME type reported by the backend, e.g. `image/jpeg`.
    pub content_type: String,
}

/// Trait implemented by every storage backend.
pub trait StorageBackend: Send + Sync {
    /// Retrieve an asset by its logical path (e.g. `"products/shoe.jpg"`).
    ///
    /// Returns `None` when the asset does not exist.
    fn get(&self, path: &str) -> impl Future<Output = anyhow::Result<Option<Asset>>> + Send;
}

/// Local filesystem storage backend (development / on-prem).
///
/// Serves files rooted at `root`.  Full implementation lives in the storage PR.
#[derive(Clone)]
pub struct LocalStorage {
    pub root: std::path::PathBuf,
}

impl LocalStorage {
    pub fn new(root: impl Into<std::path::PathBuf>) -> Self {
        Self { root: root.into() }
    }
}

impl StorageBackend for LocalStorage {
    async fn get(&self, _path: &str) -> anyhow::Result<Option<Asset>> {
        // TODO: implemented in the storage PR
        anyhow::bail!("LocalStorage::get not yet implemented")
    }
}
