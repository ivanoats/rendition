//! LocalStack-backed integration tests for `rendition::storage::S3Storage`.
//!
//! Every test in this file is gated with `#[ignore]` so the default
//! `cargo test` loop stays fast. Run explicitly with:
//!
//! ```ignore
//! cargo test --test s3_integration -- --ignored
//! ```
//!
//! A Docker daemon must be running locally — the test harness spawns a
//! LocalStack container via `testcontainers-modules` and shares it
//! across all tests in this binary via a `OnceCell`.

use std::sync::OnceLock;

use rendition::storage::{S3Storage, StorageBackend, StorageError};
use testcontainers_modules::localstack::LocalStack;
use testcontainers_modules::testcontainers::runners::AsyncRunner;
use testcontainers_modules::testcontainers::ContainerAsync;
use tokio::sync::OnceCell;
use uuid::Uuid;

// ---------------------------------------------------------------------------
// Shared container (one per test binary run)
// ---------------------------------------------------------------------------

/// Lazily-initialised LocalStack container. Started on first use and
/// dropped when the test binary exits (`testcontainers` handles Docker
/// cleanup via its `Drop` impl).
fn container_cell() -> &'static OnceCell<ContainerAsync<LocalStack>> {
    static CELL: OnceLock<OnceCell<ContainerAsync<LocalStack>>> = OnceLock::new();
    CELL.get_or_init(OnceCell::new)
}

async fn endpoint() -> String {
    let container = container_cell()
        .get_or_init(|| async {
            LocalStack::default()
                .start()
                .await
                .expect("failed to start LocalStack container")
        })
        .await;
    let host = container
        .get_host()
        .await
        .expect("failed to read container host");
    let port = container
        .get_host_port_ipv4(4566)
        .await
        .expect("failed to read container port");
    format!("http://{host}:{port}")
}

async fn fresh_bucket_and_storage() -> (String, S3Storage) {
    let ep = endpoint().await;
    let bucket = format!("rendition-test-{}", Uuid::new_v4().simple());

    // Create the bucket via a dedicated SDK client — we deliberately
    // avoid exposing bucket-creation from S3Storage (it's a read-only
    // consumer per the unit definition).
    create_bucket(&ep, &bucket).await;

    let storage = S3Storage::new_for_test(&ep, "test", "test", bucket.clone())
        .await
        .expect("S3Storage::new_for_test");
    (bucket, storage)
}

async fn create_bucket(endpoint: &str, bucket: &str) {
    use aws_config::{BehaviorVersion, Region};
    use aws_credential_types::Credentials;
    let creds = Credentials::new("test", "test", None, None, "rendition-test-setup");
    let sdk = aws_config::defaults(BehaviorVersion::latest())
        .region(Region::new("us-east-1"))
        .endpoint_url(endpoint)
        .credentials_provider(creds)
        .load()
        .await;
    let client = aws_sdk_s3::Client::new(&sdk);
    client
        .create_bucket()
        .bucket(bucket)
        .send()
        .await
        .expect("create_bucket");
}

async fn put_fixture(endpoint: &str, bucket: &str, key: &str, body: &[u8], content_type: &str) {
    use aws_config::{BehaviorVersion, Region};
    use aws_credential_types::Credentials;
    use aws_sdk_s3::primitives::ByteStream;
    let creds = Credentials::new("test", "test", None, None, "rendition-test-setup");
    let sdk = aws_config::defaults(BehaviorVersion::latest())
        .region(Region::new("us-east-1"))
        .endpoint_url(endpoint)
        .credentials_provider(creds)
        .load()
        .await;
    let client = aws_sdk_s3::Client::new(&sdk);
    client
        .put_object()
        .bucket(bucket)
        .key(key)
        .body(ByteStream::from(body.to_vec()))
        .content_type(content_type)
        .send()
        .await
        .expect("put_object");
}

// ---------------------------------------------------------------------------
// Tests — each one asserts a single acceptance criterion from the unit
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "requires LocalStack — run with `cargo test -- --ignored`"]
async fn get_fetches_existing_object() {
    let ep = endpoint().await;
    let (bucket, storage) = fresh_bucket_and_storage().await;
    let body = b"hello, rendition";
    put_fixture(&ep, &bucket, "hello.txt", body, "text/plain").await;

    let asset = storage.get("hello.txt").await.expect("get should succeed");
    assert_eq!(asset.data, body);
    assert_eq!(asset.size, body.len());
    assert_eq!(asset.content_type, "text/plain");
}

#[tokio::test]
#[ignore = "requires LocalStack — run with `cargo test -- --ignored`"]
async fn get_returns_not_found_for_missing_key() {
    let (_bucket, storage) = fresh_bucket_and_storage().await;
    let err = storage.get("does-not-exist.jpg").await.unwrap_err();
    assert_eq!(err, StorageError::NotFound);
}

#[tokio::test]
#[ignore = "requires LocalStack — run with `cargo test -- --ignored`"]
async fn exists_true_for_present_object() {
    let ep = endpoint().await;
    let (bucket, storage) = fresh_bucket_and_storage().await;
    put_fixture(&ep, &bucket, "here.png", b"x", "image/png").await;

    assert!(storage.exists("here.png").await.unwrap());
}

#[tokio::test]
#[ignore = "requires LocalStack — run with `cargo test -- --ignored`"]
async fn exists_false_for_missing_object() {
    let (_bucket, storage) = fresh_bucket_and_storage().await;
    assert!(!storage.exists("ghost.png").await.unwrap());
}

#[tokio::test]
#[ignore = "requires LocalStack — run with `cargo test -- --ignored`"]
async fn get_range_returns_only_requested_bytes() {
    let ep = endpoint().await;
    let (bucket, storage) = fresh_bucket_and_storage().await;
    let full: Vec<u8> = (0u8..=255).collect();
    put_fixture(&ep, &bucket, "bytes.bin", &full, "application/octet-stream").await;

    let asset = storage
        .get_range("bytes.bin", 10..50)
        .await
        .expect("get_range should succeed");
    assert_eq!(asset.data.len(), 40);
    assert_eq!(asset.data[0], 10);
    assert_eq!(asset.data[39], 49);
    assert_eq!(asset.size, 40);
}

#[tokio::test]
#[ignore = "requires LocalStack — run with `cargo test -- --ignored`"]
async fn get_range_rejects_inverted_range() {
    let (_bucket, storage) = fresh_bucket_and_storage().await;
    // Construct the inverted range at runtime so clippy's
    // `reversed_empty_ranges` lint doesn't fire on a literal.
    let start: u64 = 20;
    let end: u64 = 10;
    let err = storage
        .get_range("anything.bin", start..end)
        .await
        .unwrap_err();
    assert!(matches!(err, StorageError::InvalidPath { .. }));
}

#[tokio::test]
#[ignore = "requires LocalStack — run with `cargo test -- --ignored`"]
async fn content_type_from_upload_metadata_is_preferred() {
    let ep = endpoint().await;
    let (bucket, storage) = fresh_bucket_and_storage().await;
    // Upload with an unusual but legitimate content type that extension
    // inference would get wrong.
    put_fixture(&ep, &bucket, "weird.jpg", b"not-really-jpeg", "image/heic").await;

    let asset = storage.get("weird.jpg").await.unwrap();
    assert_eq!(asset.content_type, "image/heic");
}

#[tokio::test]
#[ignore = "requires LocalStack — run with `cargo test -- --ignored`"]
async fn content_type_falls_back_to_extension_when_octet_stream() {
    let ep = endpoint().await;
    let (bucket, storage) = fresh_bucket_and_storage().await;
    put_fixture(
        &ep,
        &bucket,
        "photo.jpg",
        b"bytes",
        "application/octet-stream",
    )
    .await;

    let asset = storage.get("photo.jpg").await.unwrap();
    // Falls back to extension inference per R-05.
    assert_eq!(asset.content_type, "image/jpeg");
}

#[tokio::test]
#[ignore = "requires LocalStack — run with `cargo test -- --ignored`"]
async fn is_healthy_true_after_successful_call() {
    let ep = endpoint().await;
    let (bucket, storage) = fresh_bucket_and_storage().await;
    put_fixture(&ep, &bucket, "ok.txt", b"x", "text/plain").await;
    let _ = storage.get("ok.txt").await.unwrap();
    assert!(storage.is_healthy());
}
