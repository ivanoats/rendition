//! Filesystem-backed storage for dev and on-prem deployments.
//!
//! `LocalStorage` reads assets from a root directory on the local
//! filesystem. It enforces a lexical path-traversal check via
//! [`safe_join`] so a caller-supplied path like `"../../etc/passwd"`
//! cannot escape the configured root.
//!
//! Reads are wrapped in `tokio::time::timeout(local_timeout_ms, …)` so a
//! hung filesystem (e.g. NFS) cannot block a request indefinitely.

use std::path::{Component, Path, PathBuf};
use std::time::Duration;

use super::{content_type_from_ext, Asset, StorageBackend, StorageError};

/// Reads assets from a directory on the local filesystem.
#[derive(Clone)]
pub struct LocalStorage {
    root: PathBuf,
    timeout: Duration,
}

impl LocalStorage {
    /// Create a new adapter rooted at `root` with a read timeout of
    /// `timeout_ms` milliseconds (0 = no timeout, not recommended).
    pub fn new(root: impl Into<PathBuf>, timeout_ms: u64) -> Self {
        Self {
            root: root.into(),
            timeout: Duration::from_millis(timeout_ms),
        }
    }
}

impl StorageBackend for LocalStorage {
    async fn get(&self, path: &str) -> Result<Asset, StorageError> {
        let full_path = safe_join(&self.root, path)?;
        let content_type = content_type_from_ext(path).to_string();
        let read_fut = tokio::fs::read(&full_path);
        let data = match tokio::time::timeout(self.timeout, read_fut).await {
            Ok(Ok(bytes)) => bytes,
            Ok(Err(err)) if err.kind() == std::io::ErrorKind::NotFound => {
                return Err(StorageError::NotFound);
            }
            Ok(Err(err)) => {
                return Err(StorageError::Other {
                    source: Box::new(err),
                });
            }
            Err(_elapsed) => {
                return Err(StorageError::Timeout { op: "get" });
            }
        };
        let size = data.len();
        Ok(Asset {
            data,
            content_type,
            size,
        })
    }

    async fn exists(&self, path: &str) -> Result<bool, StorageError> {
        let full_path = match safe_join(&self.root, path) {
            Ok(p) => p,
            // Path-traversal attempts count as "does not exist" per
            // the in-domain contract: the caller can't reach the file,
            // which for all semantic purposes means it's absent.
            Err(StorageError::InvalidPath { .. }) => return Ok(false),
            Err(other) => return Err(other),
        };
        let metadata_fut = tokio::fs::metadata(full_path);
        match tokio::time::timeout(self.timeout, metadata_fut).await {
            Ok(Ok(_)) => Ok(true),
            Ok(Err(err)) if err.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Ok(Err(err)) => Err(StorageError::Other {
                source: Box::new(err),
            }),
            Err(_elapsed) => Err(StorageError::Timeout { op: "exists" }),
        }
    }
}

// ---------------------------------------------------------------------------
// Path-safety helper
// ---------------------------------------------------------------------------

/// Validate that `path` is a safe relative filesystem path and join it
/// with `root`.
///
/// Rejects paths that are absolute or that contain any non-
/// [`Component::Normal`] segment (e.g. `..`, `.`, or a Windows drive
/// prefix). This prevents directory-traversal attacks where a caller
/// supplies a path like `"../../etc/passwd"` to escape the storage root.
///
/// **Limitation:** this check is purely lexical. Symlinks inside the
/// root that point outside it are not followed or verified here. If the
/// underlying storage needs to defend against symlink-based escapes,
/// callers should additionally canonicalize the resolved path and verify
/// it remains under `root`.
fn safe_join(root: &Path, path: &str) -> Result<PathBuf, StorageError> {
    let rel = Path::new(path);
    if rel.is_absolute() {
        return Err(StorageError::InvalidPath {
            reason: format!("absolute path not allowed: {path}"),
        });
    }
    for component in rel.components() {
        match component {
            Component::Normal(_) => {}
            _ => {
                return Err(StorageError::InvalidPath {
                    reason: format!("invalid path component in: {path}"),
                });
            }
        }
    }
    Ok(root.join(rel))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn make_storage(dir: &TempDir) -> LocalStorage {
        LocalStorage::new(dir.path(), 2000)
    }

    #[tokio::test]
    async fn exists_returns_true_for_present_file() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("test.jpg"), b"data").unwrap();
        let storage = make_storage(&dir);
        assert!(storage.exists("test.jpg").await.unwrap());
    }

    #[tokio::test]
    async fn exists_returns_false_for_missing_file() {
        let dir = TempDir::new().unwrap();
        let storage = make_storage(&dir);
        assert!(!storage.exists("ghost.jpg").await.unwrap());
    }

    #[tokio::test]
    async fn get_returns_correct_bytes_and_content_type() {
        let dir = TempDir::new().unwrap();
        let payload = b"fake jpeg payload";
        fs::write(dir.path().join("photo.jpg"), payload).unwrap();
        let storage = make_storage(&dir);
        let asset = storage.get("photo.jpg").await.unwrap();
        assert_eq!(asset.data, payload);
        assert_eq!(asset.content_type, "image/jpeg");
        assert_eq!(asset.size, payload.len());
    }

    #[tokio::test]
    async fn get_returns_correct_content_type_for_png() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("banner.png"), b"png data").unwrap();
        let storage = make_storage(&dir);
        let asset = storage.get("banner.png").await.unwrap();
        assert_eq!(asset.content_type, "image/png");
    }

    #[tokio::test]
    async fn get_returns_not_found_for_missing_file() {
        let dir = TempDir::new().unwrap();
        let storage = make_storage(&dir);
        assert_eq!(
            storage.get("ghost.jpg").await.unwrap_err(),
            StorageError::NotFound
        );
    }

    #[tokio::test]
    async fn get_size_matches_file_length() {
        let dir = TempDir::new().unwrap();
        let data = vec![0u8; 1024];
        fs::write(dir.path().join("big.jpg"), &data).unwrap();
        let storage = make_storage(&dir);
        let asset = storage.get("big.jpg").await.unwrap();
        assert_eq!(asset.size, 1024);
        assert_eq!(asset.data.len(), 1024);
    }

    // -- Directory traversal prevention -------------------------------------

    #[tokio::test]
    async fn get_rejects_dotdot_path() {
        let dir = TempDir::new().unwrap();
        let storage = make_storage(&dir);
        assert!(matches!(
            storage.get("../etc/passwd").await.unwrap_err(),
            StorageError::InvalidPath { .. }
        ));
    }

    #[tokio::test]
    async fn get_rejects_absolute_path() {
        let dir = TempDir::new().unwrap();
        let storage = make_storage(&dir);
        assert!(matches!(
            storage.get("/etc/passwd").await.unwrap_err(),
            StorageError::InvalidPath { .. }
        ));
    }

    #[tokio::test]
    async fn exists_returns_false_for_dotdot_path() {
        let dir = TempDir::new().unwrap();
        let storage = make_storage(&dir);
        assert!(!storage.exists("../secret.txt").await.unwrap());
    }

    #[tokio::test]
    async fn exists_returns_false_for_absolute_path() {
        let dir = TempDir::new().unwrap();
        let storage = make_storage(&dir);
        assert!(!storage.exists("/etc/passwd").await.unwrap());
    }

    // -- get_range via default trait impl -----------------------------------

    #[tokio::test]
    async fn get_range_slices_local_file() {
        let dir = TempDir::new().unwrap();
        let data: Vec<u8> = (0u8..=255).collect();
        fs::write(dir.path().join("bytes.bin"), &data).unwrap();
        let storage = make_storage(&dir);
        let asset = storage.get_range("bytes.bin", 10..50).await.unwrap();
        assert_eq!(asset.data.len(), 40);
        assert_eq!(asset.data[0], 10);
        assert_eq!(asset.data[39], 49);
    }

    #[tokio::test]
    async fn get_range_rejects_inverted_range() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("a.bin"), b"12345").unwrap();
        let storage = make_storage(&dir);
        // Construct the inverted range at runtime so clippy's
        // `reversed_empty_ranges` lint doesn't flag a literal.
        let start: u64 = 10;
        let end: u64 = 5;
        let range = start..end;
        assert!(matches!(
            storage.get_range("a.bin", range).await.unwrap_err(),
            StorageError::InvalidPath { .. }
        ));
    }

    #[test]
    fn safe_join_accepts_normal_path() {
        let root = std::path::Path::new("/tmp/root");
        assert!(safe_join(root, "products/shoe.jpg").is_ok());
    }

    #[test]
    fn safe_join_rejects_dotdot() {
        let root = std::path::Path::new("/tmp/root");
        assert!(matches!(
            safe_join(root, "../etc/passwd").unwrap_err(),
            StorageError::InvalidPath { .. }
        ));
    }

    #[test]
    fn safe_join_rejects_absolute() {
        let root = std::path::Path::new("/tmp/root");
        assert!(matches!(
            safe_join(root, "/etc/passwd").unwrap_err(),
            StorageError::InvalidPath { .. }
        ));
    }
}
