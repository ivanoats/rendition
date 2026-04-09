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
